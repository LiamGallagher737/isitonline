DROP TABLE IF EXISTS monitors;
DROP TABLE IF EXISTS checks;
DROP TABLE IF EXISTS changes;

CREATE TABLE monitors(
    monitor_id INTEGER PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    target TEXT NOT NULL,
    cron TEXT NOT NULL
);

CREATE TABLE checks(
    timestamp TIMESTAMP DEFAULT CURRENT_TIMESTAMP NOT NULL,
    success BOOLEAN NOT NULL,
    monitor_id INTEGER NOT NULL,
    FOREIGN KEY (monitor_id)
       REFERENCES monitors (monitor_id)
);

CREATE INDEX idx_checks_monitor_id_timestamp ON checks(monitor_id, timestamp);

CREATE TABLE changes(
    timestamp TIMESTAMP DEFAULT CURRENT_TIMESTAMP NOT NULL,
    success BOOLEAN NOT NULL,
    http_code INTEGER,
    monitor_id INTEGER NOT NULL,
    FOREIGN KEY (monitor_id)
       REFERENCES monitors (monitor_id)
);
