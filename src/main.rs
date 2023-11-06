use actix_files::Files;
use actix_web::error::{Error, ErrorInternalServerError};
use actix_web::middleware::{Compress, Logger};
use actix_web::web::{Data, Form, Path};
use actix_web::{delete, get, post, App, HttpServer};
use askama::Template;
use async_cron_scheduler::{Job, JobId, Scheduler};
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

    let monitors = sqlx::query!("SELECT monitor_id, name, target, cron FROM monitors")
        .fetch_all(&pool)
        .await
        .expect("Failed to fetch existing monitors");

    let http_client = reqwest::Client::new();

    info!("Scheduling {} existing monitors", monitors.len());
    let mut job_ids = BTreeMap::new();
    for monitor in monitors {
        let http_client = http_client.clone();
        let pool = pool.clone();
        let job_id = scheduler.insert(Job::cron(&monitor.cron).unwrap(), move |_| {
            // info!("Checking {} at {}", monitor.name, monitor.target);
            actix_web::rt::spawn(update_monitor(
                monitor.monitor_id,
                monitor.target.clone(),
                http_client.clone(),
                pool.clone(),
            ));
        });
        job_ids.insert(monitor.monitor_id, job_id);
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
    let success = match res {
        Ok(res) => !res.status().is_client_error() && !res.status().is_server_error(),
        Err(_) => false,
    };
    let insertion = sqlx::query!(
        "INSERT INTO checks (success, monitor_id) VALUES (?, ?)",
        success,
        id,
    )
    .execute(&pool)
    .await;
    // info!("{target} is {}", if success { "online" } else { "offline" });
    if let Err(err) = insertion {
        error!("Failed to insert check into database for {target}. Error: {err}");
    }
}

async fn get_monitors(pool: &SqlitePool) -> Result<Vec<MonitorTemplate>, sqlx::Error> {
    sqlx::query!(
        r#"
        SELECT
            m.monitor_id,
            m.name,
            IFNULL((
                SELECT success
                FROM checks c
                WHERE c.monitor_id = m.monitor_id
                ORDER BY c.timestamp DESC
                LIMIT 1
            ), 2) status,
            (
                SELECT MAX(timestamp)
                FROM checks c
                WHERE c.monitor_id = m.monitor_id
            ) timestamp
        FROM monitors m
    "#
    )
    .fetch_all(pool)
    .await
    .map(|rows| {
        rows.iter()
            .map(|row| MonitorTemplate {
                id: row.monitor_id.unwrap(),
                name: row.name.clone(),
                status: match row.status {
                    Some(0) => Status::Offline,
                    Some(1) => Status::Online,
                    Some(2) => Status::Pending,
                    _ => unreachable!(),
                },
                timestamp: row.timestamp.clone(),
            })
            .collect()
    })
}

#[get("/")]
async fn index(data: Data<AppData>) -> Result<IndexTemplate, Error> {
    let monitors = get_monitors(&data.pool)
        .await
        .map_err(ErrorInternalServerError)?;
    Ok(IndexTemplate { monitors })
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
        "INSERT INTO monitors (name, target, cron) VALUES(?, ?, ?) RETURNING monitor_id",
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
                row.monitor_id,
                params.target.clone(),
                http_client.clone(),
                pool.clone(),
            ));
        });
    data.job_ids.lock().await.insert(row.monitor_id, job_id);

    Ok(MonitorTemplate {
        id: row.monitor_id,
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
        DELETE FROM monitors WHERE monitor_id = ?;
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
    monitors: Vec<MonitorTemplate>,
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

enum Status {
    Online,
    Offline,
    Pending,
}
