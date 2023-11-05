use actix_files::Files;
use actix_web::error::{Error, ErrorInternalServerError};
use actix_web::middleware::{Compress, Logger};
use actix_web::web::{Data, Form};
use actix_web::{get, post, App, HttpServer};
use askama::Template;
use async_cron_scheduler::{Job, Scheduler};
use env_logger::Env;
use futures::lock::Mutex;
use log::{error, info};
use serde::Deserialize;
use sqlx::SqlitePool;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

#[derive(Clone)]
struct AppData {
    pool: SqlitePool,
    scheduler: Arc<Mutex<Scheduler<chrono::Local>>>,
    http_client: reqwest::Client,
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
    for monitor in monitors {
        let http_client = http_client.clone();
        let pool = pool.clone();
        scheduler.insert(Job::cron(&monitor.cron).unwrap(), move |_| {
            info!("Checking {} at {}", monitor.name, monitor.target);
            actix_web::rt::spawn(update_monitor(
                monitor.monitor_id,
                monitor.target.clone(),
                http_client.clone(),
                pool.clone(),
            ));
        });
    }

    info!("Serving on http://{ip}:{port}");

    let app_data = AppData {
        pool: pool.clone(),
        scheduler: Arc::new(Mutex::new(scheduler)),
        http_client,
    };

    let server = HttpServer::new(move || {
        App::new()
            .wrap(Logger::default())
            .wrap(Logger::new("%a %{User-Agent}i"))
            .wrap(Compress::default())
            .app_data(Data::new(app_data.clone()))
            .service(index)
            .service(monitors_get)
            .service(monitor_post)
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
    info!("{target} is {}", if success { "online" } else { "offline" });
    if insertion.is_err() {
        error!("Failed to insert check into database for {target}");
    }
}

async fn get_monitors(pool: &SqlitePool) -> Result<Vec<MonitorTemplate>, sqlx::Error> {
    sqlx::query!(
        r#"
        SELECT
        m.name,
        IFNULL((
            SELECT success
            FROM checks c
            WHERE c.monitor_id = m.monitor_id
            ORDER BY timestamp DESC
            LIMIT 1
        ), 2) status
        FROM monitors m
    "#
    )
    .fetch_all(pool)
    .await
    .map(|rows| {
        rows.iter()
            .map(|row| MonitorTemplate {
                name: row.name.clone(),
                status: match row.status.unwrap() {
                    0 => Status::Offline,
                    1 => Status::Online,
                    2 => Status::Pending,
                    _ => unreachable!(),
                },
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

#[derive(Deserialize)]
struct MonitorPostParams {
    name: String,
    target: String,
    cron: String,
}

#[get("/monitors")]
async fn monitors_get(data: Data<AppData>) -> Result<MonitorsTemplate, Error> {
    let monitors = get_monitors(&data.pool)
        .await
        .map_err(ErrorInternalServerError)?;
    Ok(MonitorsTemplate { monitors })
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
    data.scheduler
        .lock()
        .await
        .insert(Job::cron(&params.cron).unwrap(), move |_| {
            info!("Checking {} at {}", params.name, params.target);
            actix_web::rt::spawn(update_monitor(
                row.monitor_id,
                params.target.clone(),
                http_client.clone(),
                pool.clone(),
            ));
        });

    Ok(MonitorTemplate {
        name,
        status: Status::Pending,
    })
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
    name: String,
    status: Status,
}

enum Status {
    Online,
    Offline,
    Pending,
}
