use actix_files::Files;
use actix_web::error::{Error, ErrorInternalServerError};
use actix_web::middleware::{Compress, Logger};
use actix_web::rt::time::interval;
use actix_web::web::{Data, Form, Path};
use actix_web::{delete, get, post, App, HttpServer};
use actix_web_lab::sse::{self, Sse};
use actix_web_lab::util::InfallibleStream;
use askama::Template;
use async_cron_scheduler::{Job, JobId, Scheduler};
use chrono::Utc;
use env_logger::Env;
use futures::lock::Mutex;
use log::{error, info, warn};
use serde::Deserialize;
use sqlx::SqlitePool;
use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

#[derive(Clone)]
struct AppData {
    pool: SqlitePool,
    scheduler: Arc<Mutex<Scheduler<chrono::Local>>>,
    http_client: reqwest::Client,
    job_ids: Arc<Mutex<BTreeMap<i64, JobId>>>,
    mub: Arc<MonitorUpdatesBroadcaster>,
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

    let mub = MonitorUpdatesBroadcaster::create();

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
        let mub = Arc::clone(&mub);
        let job_id = scheduler.insert(Job::cron(&monitor.cron).unwrap(), move |_| {
            // info!("Checking {} at {}", monitor.name, monitor.target);
            actix_web::rt::spawn(update_monitor(
                monitor.id,
                monitor.name.clone(),
                monitor.target.clone(),
                http_client.clone(),
                pool.clone(),
                mub.clone(),
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
        mub: Arc::clone(&mub),
    };

    let server = HttpServer::new(move || {
        App::new()
            .wrap(Logger::new("%a %{User-Agent}i"))
            .wrap(Compress::default())
            .app_data(Data::new(app_data.clone()))
            .service(index)
            .service(monitor_updates_sse_get)
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

async fn update_monitor(
    id: i64,
    name: String,
    target: String,
    http_client: reqwest::Client,
    pool: SqlitePool,
    mub: Arc<MonitorUpdatesBroadcaster>,
) {
    let time = std::time::Instant::now();
    let res = http_client.get(&target).send().await;
    let rtt = time.elapsed().as_millis() as i64;
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
        warn!(
            "[CHANGE]: {:?} -> {:?} - {target}",
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
        "INSERT INTO response_times (time, monitor_id) VALUES (?, ?)",
        rtt,
        id
    )
    .execute(&pool)
    .await;

    if let Err(err) = insertion {
        error!("Failed to insert response time into database for {target}. Error: {err}");
    }

    let clients = mub.inner.lock().await.clients.clone();
    let send_futures = clients.iter().map(|client| {
        client.send(sse::Event::Data(
            sse::Data::new(
                MonitorTemplate {
                    id,
                    name: name.clone(),
                    status: if success {
                        Status::Online
                    } else {
                        Status::Offline
                    },
                    timestamp: Some(String::from("Just Now")),
                    response_time: Some(rtt),
                    http_code,
                }
                .render()
                .unwrap(),
            )
            .event(format!("monitor-{id}")),
        ))
    });
    let _ = futures::future::join_all(send_futures).await;
}

async fn get_monitors(pool: &SqlitePool) -> Result<Vec<MonitorTemplate>, sqlx::Error> {
    sqlx::query!("SELECT * FROM monitors_summary")
        .fetch_all(pool)
        .await
        .map(|rows| {
            rows.iter()
                .map(|row| MonitorTemplate {
                    id: row.monitor_id,
                    name: row.name.clone(),
                    status: match row.success {
                        Some(false) => Status::Offline,
                        Some(true) => Status::Online,
                        None => Status::Pending,
                    },
                    timestamp: Some(format_duration(Utc::now() - row.timestamp.and_utc())),
                    response_time: row.response_time,
                    http_code: row.http_code,
                })
                .collect()
        })
}

fn format_duration(duration: chrono::Duration) -> String {
    if duration.num_seconds() == 0 {
        String::from("Just now")
    } else if duration.num_seconds() == 1 {
        String::from("1 second ago")
    } else if duration.num_seconds() < 60 {
        format!("{} seconds ago", duration.num_seconds())
    } else if duration.num_minutes() == 1 {
        String::from("1 minute ago")
    } else if duration.num_minutes() < 60 {
        format!("{} minutes ago", duration.num_minutes())
    } else if duration.num_days() == 1 {
        String::from("yesterday")
    } else {
        format!("{} days ago", duration.num_days())
    }
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

#[get("/monitor-updates")]
async fn monitor_updates_sse_get(
    data: Data<AppData>,
) -> Sse<InfallibleStream<ReceiverStream<sse::Event>>> {
    data.mub.new_client().await
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
) -> Result<MonitorsTemplate, Error> {
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
    let mub = Arc::clone(&data.mub);
    let job_id = data
        .scheduler
        .lock()
        .await
        .insert(Job::cron(&params.cron).unwrap(), move |_| {
            // info!("Checking {} at {}", params.name, params.target);
            actix_web::rt::spawn(update_monitor(
                row.id,
                params.name.clone(),
                params.target.clone(),
                http_client.clone(),
                pool.clone(),
                mub.clone(),
            ));
        });
    data.job_ids.lock().await.insert(row.id, job_id);

    Ok(MonitorsTemplate {
        monitors: vec![MonitorTemplate {
            id: row.id,
            name,
            status: Status::Pending,
            timestamp: None,
            response_time: None,
            http_code: None,
        }],
    })
}

#[delete("/monitor/{id}")]
async fn monitor_delete(data: Data<AppData>, path: Path<i64>) -> Result<&'static str, Error> {
    let id = path.into_inner();
    sqlx::query!(
        r#"
        BEGIN TRANSACTION;
        DELETE FROM changes WHERE monitor_id = ?;
        DELETE FROM response_times WHERE monitor_id = ?;
        DELETE FROM monitors_summary WHERE monitor_id = ?;
        DELETE FROM monitors WHERE id = ?;
        COMMIT;
    "#,
        id,
        id,
        id,
        id,
    )
    .execute(&data.pool)
    .await
    .map_err(ErrorInternalServerError)?;
    data.scheduler
        .lock()
        .await
        .remove(
            data.job_ids
                .lock()
                .await
                .remove(&id)
                .ok_or(ErrorInternalServerError(
                    "Failed to remove monitor job from schedule",
                ))?,
        );
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
    response_time: Option<i64>,
    http_code: Option<i64>,
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

pub struct MonitorUpdatesBroadcaster {
    inner: Mutex<MonitorUpdatesBroadcasterInner>,
}

#[derive(Debug, Clone, Default)]
struct MonitorUpdatesBroadcasterInner {
    clients: Vec<mpsc::Sender<sse::Event>>,
}

impl MonitorUpdatesBroadcaster {
    /// Constructs new broadcaster and spawns ping loop.
    pub fn create() -> Arc<Self> {
        let this = Arc::new(MonitorUpdatesBroadcaster {
            inner: Mutex::new(MonitorUpdatesBroadcasterInner::default()),
        });
        MonitorUpdatesBroadcaster::spawn_ping(Arc::clone(&this));
        this
    }

    /// Pings clients every 10 seconds to see if they are alive and remove them from the broadcast
    /// list if not.
    fn spawn_ping(this: Arc<Self>) {
        actix_web::rt::spawn(async move {
            let mut interval = interval(Duration::from_secs(10));

            loop {
                interval.tick().await;
                this.remove_stale_clients().await;
            }
        });
    }

    /// Removes all non-responsive clients from broadcast list.
    async fn remove_stale_clients(&self) {
        let clients = self.inner.lock().await.clients.clone();

        let mut ok_clients = Vec::new();

        for client in clients {
            let send = &client.send(sse::Event::Comment("ping".into())).await;
            if send.is_ok() {
                ok_clients.push(client.clone());
            } else {
                warn!("Removing client: {:#?}", send.as_ref().unwrap_err());
            }
        }

        self.inner.lock().await.clients = ok_clients;
    }

    /// Registers client with broadcaster, returning an SSE response body.
    pub async fn new_client(&self) -> Sse<InfallibleStream<ReceiverStream<sse::Event>>> {
        let (tx, rx) = mpsc::channel(10);

        tx.send(sse::Data::new("connected").into()).await.unwrap();

        self.inner.lock().await.clients.push(tx);

        Sse::from_infallible_receiver(rx)
    }

    /// Broadcasts `msg` to all clients.
    pub async fn broadcast(&self, msg: &str) {
        let clients = self.inner.lock().await.clients.clone();

        let send_futures = clients
            .iter()
            .map(|client| client.send(sse::Data::new(msg).into()));

        let _ = futures::future::join_all(send_futures).await;
    }
}
