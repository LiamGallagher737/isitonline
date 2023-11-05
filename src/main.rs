use actix_files::Files;
use actix_web::error::{Error, ErrorInternalServerError};
use actix_web::web::{Data, Form};
use actix_web::{get, middleware::Logger, post, App, HttpServer};
use askama::Template;
use async_cron_scheduler::{Job, Scheduler};
use env_logger::Env;
use futures::lock::Mutex;
use log::info;
use serde::Deserialize;
use sqlx::SqlitePool;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

#[derive(Clone)]
struct AppData {
    pool: SqlitePool,
    scheduler: Arc<Mutex<Scheduler<chrono::Local>>>,
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
    let monitors = sqlx::query!("SELECT name, target, cron FROM monitors")
        .fetch_all(&pool)
        .await
        .expect("Failed to fetch existing monitors");
    info!("Scheduling {} existing monitors", monitors.len());
    for monitor in monitors {
        scheduler.insert(Job::cron(&monitor.cron).unwrap(), move |_| {
            info!("Checking {} at {}", monitor.name, monitor.target);
        });
    }

    info!("Serving on http://{ip}:{port}");

    let app_data = AppData {
        pool: pool.clone(),
        scheduler: Arc::new(Mutex::new(scheduler)),
    };

    let server = HttpServer::new(move || {
        App::new()
            .wrap(Logger::default())
            .wrap(Logger::new("%a %{User-Agent}i"))
            .app_data(Data::new(app_data.clone()))
            .service(index)
            .service(monitor_post)
            .service(Files::new("/static", "./static"))
    })
    .bind((ip, port))?
    .run();

    let (server_res, _sched_res) = futures::future::join(server, sched_service).await;
    server_res
}

#[get("/")]
async fn index(data: Data<AppData>) -> Result<IndexTemplate, Error> {
    let monitors = sqlx::query!("SELECT rowid, name FROM monitors ORDER BY rowid")
        .fetch_all(&data.pool)
        .await
        .map_err(ErrorInternalServerError)?;

    Ok(IndexTemplate {
        monitors: monitors.iter().map(|row| row.name.clone()).collect(),
    })
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
    sqlx::query!(
        "INSERT INTO monitors VALUES(?, ?, ?)",
        params.name,
        params.target,
        params.cron,
    )
    .execute(&data.pool)
    .await
    .map_err(ErrorInternalServerError)?;

    let name = params.name.clone();
    data.scheduler
        .lock()
        .await
        .insert(Job::cron(&params.cron).unwrap(), move |_| {
            info!("Checking {} at {}", params.name, params.target);
        });

    Ok(MonitorTemplate { name })
}

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate {
    monitors: Vec<String>,
}

#[derive(Template)]
#[template(path = "monitor.html")]
struct MonitorTemplate {
    name: String,
}
