DROP TABLE IF EXISTS checks;
DROP TABLE IF EXISTS changes;
DROP TABLE IF EXISTS monitors;

CREATE TABLE monitors(
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    target TEXT NOT NULL,
    cron TEXT NOT NULL
);

CREATE TABLE checks(
    timestamp TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    success BOOLEAN NOT NULL,
    monitor_id INTEGER NOT NULL,
    FOREIGN KEY (monitor_id)
       REFERENCES monitors (id)
);

CREATE INDEX idx_checks_monitor_id_timestamp ON checks(monitor_id, timestamp);

CREATE TABLE changes(
    timestamp TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    success BOOLEAN NOT NULL,
    http_code INTEGER,
    monitor_id INTEGER NOT NULL,
    FOREIGN KEY (monitor_id)
       REFERENCES monitors (id)
);
