use actix_files::Files;
use actix_web::error::{Error, ErrorInternalServerError};
use actix_web::middleware::{Compress, Logger};
use actix_web::web::{Data, Form, Path};
use actix_web::{delete, get, post, App, HttpServer};
use askama::Template;
use async_cron_scheduler::{Job, JobId, Scheduler};
use chrono::{TimeZone, Utc};
use env_logger::Env;
use futures::lock::Mutex;
use log::{error, info};
use serde::Deserialize;
use sqlx::SqlitePool;
use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

#[derive(Clone)]
struct AppData {
    pool: SqlitePool,
    scheduler: Arc<Mutex<Scheduler<chrono::Local>>>,
    http_client: reqwest::Client,
    job_ids: Arc<Mutex<BTreeMap<i64, JobId>>>,
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env_logger::init_from_env(Env::default().default_filter_or("info"));

    info!("Connecting to db");
    let pool = SqlitePool::connect("sqlite:db/data.db")
        .await
        .expect("Failed to connect to db");

    // Config
    let ip = if let Ok(var) = std::env::var("IP") {
        var.parse::<IpAddr>()
            .expect("Failed to parse enviroment variable 'IP' to ip address")
    } else {
        IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))
    };
    let port = if let Ok(var) = std::env::var("PORT") {
        var.parse::<u16>()
            .expect("Failed to parse enviroment variable 'PORT' to number")
    } else {
        8080
    };

    info!("Setting up schedule");
    let (mut scheduler, sched_service) =
        Scheduler::<chrono::Local>::launch(actix_web::rt::time::sleep);

    let monitors = sqlx::query!("SELECT id, name, target, cron FROM monitors")
        .fetch_all(&pool)
        .await
        .expect("Failed to fetch existing monitors");

    let http_client = reqwest::ClientBuilder::new()
        .user_agent("https://github.com/LiamGallagher737/isitonline")
        .build()
        .unwrap();

    info!("Scheduling {} existing monitors", monitors.len());
    let mut job_ids = BTreeMap::new();
    for monitor in monitors {
        let http_client = http_client.clone();
        let pool = pool.clone();
        let job_id = scheduler.insert(Job::cron(&monitor.cron).unwrap(), move |_| {
            // info!("Checking {} at {}", monitor.name, monitor.target);
            actix_web::rt::spawn(update_monitor(
                monitor.id,
                monitor.target.clone(),
                http_client.clone(),
                pool.clone(),
            ));
        });
        job_ids.insert(monitor.id, job_id);
    }

    info!("Serving on http://{ip}:{port}");

    let app_data = AppData {
        pool: pool.clone(),
        scheduler: Arc::new(Mutex::new(scheduler)),
        http_client,
        job_ids: Arc::new(Mutex::new(job_ids)),
    };

    let server = HttpServer::new(move || {
        App::new()
            .wrap(Logger::new("%a %{User-Agent}i"))
            .wrap(Compress::default())
            .app_data(Data::new(app_data.clone()))
            .service(index)
            .service(monitors_get)
            .service(monitor_post)
            .service(monitor_delete)
            .service(Files::new("/static", "./static"))
    })
    .bind((ip, port))?
    .run();

    let (server_res, _sched_res) = futures::future::join(server, sched_service).await;
    server_res
}

async fn update_monitor(id: i64, target: String, http_client: reqwest::Client, pool: SqlitePool) {
    let res = http_client.get(&target).send().await;
    let success = match res.as_ref() {
        Ok(res) => !res.status().is_client_error() && !res.status().is_server_error(),
        Err(_) => false,
    };

    let last_change =
        sqlx::query!("SELECT success, http_code FROM changes WHERE monitor_id = ? ORDER BY timestamp DESC LIMIT 1", id)
            .fetch_optional(&pool)
            .await;

    let http_code = res.map(|res| res.status().as_u16() as i64).ok();
    let insert_change = match last_change.as_ref() {
        Ok(None) => true,
        Ok(Some(last_change)) if last_change.success != success => true,
        Ok(Some(last_change)) if last_change.http_code != http_code => true,
        Err(err) => {
            error!("Failed to select last change for {target}. Error: {err}");
            false
        }
        _ => false,
    };

    if insert_change {
        println!(
            "{:?} -> {:?} - {target}",
            last_change.map(|r| r.map(|r| r.http_code)),
            &http_code
        );
    }

    if insert_change {
        let insertion = sqlx::query!(
            "INSERT INTO changes (success, http_code, monitor_id) VALUES (?,?,?)",
            success,
            http_code,
            id
        )
        .execute(&pool)
        .await;

        if let Err(err) = insertion {
            error!("Failed to insert change into database for {target}. Error: {err}");
        }
    }

    let insertion = sqlx::query!(
        "INSERT INTO checks (success, monitor_id) VALUES (?, ?)",
        success,
        id,
    )
    .execute(&pool)
    .await;

    if let Err(err) = insertion {
        error!("Failed to insert check into database for {target}. Error: {err}");
    }
}

async fn get_monitors(pool: &SqlitePool) -> Result<Vec<MonitorTemplate>, sqlx::Error> {
    sqlx::query!(
        r#"
        SELECT
            m.id,
            m.name,
            IFNULL((
                SELECT success
                FROM checks c
                WHERE c.monitor_id = m.id
                ORDER BY c.timestamp DESC
                LIMIT 1
            ), 2) "status: i64",
            (
                SELECT unixepoch(MAX(timestamp))
                FROM checks c
                WHERE c.monitor_id = m.id
            ) "timestamp: i64"
        FROM monitors m
    "#
    )
    .fetch_all(pool)
    .await
    .map(|rows| {
        rows.iter()
            .map(|row| MonitorTemplate {
                id: row.id.unwrap(),
                name: row.name.clone(),
                status: match row.status {
                    Some(0) => Status::Offline,
                    Some(1) => Status::Online,
                    Some(2) => Status::Pending,
                    _ => unreachable!(),
                },
                timestamp: Some(format_timestamp(row.timestamp.unwrap())),
            })
            .collect()
    })
}

fn format_timestamp(timestamp: i64) -> String {
    let utc = Utc.timestamp_opt(timestamp, 0).unwrap();
    let local = utc.with_timezone(&chrono::Local);
    local.format("%b, %d %Y • %H:%M:%S").to_string()
}

#[get("/")]
async fn index(data: Data<AppData>) -> Result<IndexTemplate, Error> {
    let monitors = get_monitors(&data.pool)
        .await
        .map_err(ErrorInternalServerError)?;

    let mut issues = sqlx::query!(
        r#"
        SELECT c.*, m.name, m.target
        FROM changes c
        JOIN monitors m ON c.monitor_id = m.id
        WHERE c.success = 0
        AND c.timestamp = (
            SELECT MAX(timestamp)
            FROM changes
            WHERE monitor_id = c.monitor_id
        );
    "#
    )
    .fetch_all(&data.pool)
    .await
    .map_err(ErrorInternalServerError)?;
    issues.sort_by(|row_a, row_b| row_b.timestamp.cmp(&row_a.timestamp));

    Ok(IndexTemplate {
        online_monitors: monitors
            .iter()
            .filter(|monitor| monitor.status == Status::Online)
            .count(),
        offline_monitors: monitors
            .iter()
            .filter(|monitor| monitor.status == Status::Offline)
            .count(),
        warnings: 404,
        average_uptime: format!("{:.1}", 98.363),
        monitors,
        issues: issues
            .iter()
            .map(|row| IssueTemplate {
                timestamp: row
                    .timestamp
                    .and_utc()
                    .with_timezone(&chrono::Local)
                    .format("%b, %d %Y • %I:%M:%S %p")
                    .to_string(),
                success: row.success,
                http_code: row.http_code.map(|code| code as u16),
                name: row.name.clone(),
                target: row.target.clone(),
            })
            .collect(),
    })
}

#[get("/monitors")]
async fn monitors_get(data: Data<AppData>) -> Result<MonitorsTemplate, Error> {
    let monitors = get_monitors(&data.pool)
        .await
        .map_err(ErrorInternalServerError)?;
    Ok(MonitorsTemplate { monitors })
}

#[derive(Deserialize)]
struct MonitorPostParams {
    name: String,
    target: String,
    cron: String,
}

#[post("/monitor")]
async fn monitor_post(
    data: Data<AppData>,
    params: Form<MonitorPostParams>,
) -> Result<MonitorTemplate, Error> {
    let row = sqlx::query!(
        "INSERT INTO monitors (name, target, cron) VALUES(?, ?, ?) RETURNING id",
        params.name,
        params.target,
        params.cron,
    )
    .fetch_one(&data.pool)
    .await
    .map_err(ErrorInternalServerError)?;

    let name = params.name.clone();
    let http_client = data.http_client.clone();
    let pool = data.pool.clone();
    let job_id = data
        .scheduler
        .lock()
        .await
        .insert(Job::cron(&params.cron).unwrap(), move |_| {
            // info!("Checking {} at {}", params.name, params.target);
            actix_web::rt::spawn(update_monitor(
                row.id,
                params.target.clone(),
                http_client.clone(),
                pool.clone(),
            ));
        });
    data.job_ids.lock().await.insert(row.id, job_id);

    Ok(MonitorTemplate {
        id: row.id,
        name,
        status: Status::Pending,
        timestamp: None,
    })
}

#[delete("/monitor/{id}")]
async fn monitor_delete(data: Data<AppData>, path: Path<i64>) -> Result<&'static str, Error> {
    let id = path.into_inner();
    sqlx::query!(
        r#"
        BEGIN TRANSACTION;
        DELETE FROM checks WHERE monitor_id = ?;
        DELETE FROM monitors WHERE id = ?;
        COMMIT;
    "#,
        id,
        id
    )
    .execute(&data.pool)
    .await
    .map_err(ErrorInternalServerError)?;
    data.scheduler
        .lock()
        .await
        .remove(data.job_ids.lock().await.remove(&id).unwrap());
    Ok("Success")
}

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate {
    online_monitors: usize,
    offline_monitors: usize,
    warnings: usize,
    average_uptime: String,
    monitors: Vec<MonitorTemplate>,
    issues: Vec<IssueTemplate>,
}

#[derive(Template)]
#[template(path = "monitors.html")]
struct MonitorsTemplate {
    monitors: Vec<MonitorTemplate>,
}

#[derive(Template)]
#[template(path = "monitor.html")]
struct MonitorTemplate {
    id: i64,
    name: String,
    status: Status,
    timestamp: Option<String>,
}

#[derive(Template)]
#[template(path = "issue.html")]
struct IssueTemplate {
    timestamp: String,
    success: bool,
    http_code: Option<u16>,
    name: String,
    target: String,
}

#[derive(PartialEq)]
enum Status {
    Offline,
    Online,
    Pending,
}
