DROP TABLE IF EXISTS monitors;
DROP TABLE IF EXISTS checks;

CREATE TABLE monitors(
    monitor_id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    target TEXT NOT NULL,
    cron TEXT NOT NULL
);

CREATE TABLE checks(
    timestamp TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    success BOOLEAN NOT NULL,
    monitor_id INTEGER NOT NULL,
    FOREIGN KEY (monitor_id)
       REFERENCES monitors (monitor_id)
);

CREATE INDEX idx_checks_monitor_id_timestamp ON checks(monitor_id, timestamp);

CREATE TABLE changes(
    timestamp TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    success BOOLEAN NOT NULL,
    monitor_id INTEGER NOT NULL,
    FOREIGN KEY (monitor_id)
       REFERENCES monitors (monitor_id)
);

CREATE TABLE archive(
    timestamp TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    average_rtt REAL NOT NULL,
    monitor_id INTEGER NOT NULL,
    FOREIGN KEY (monitor_id)
       REFERENCES monitors (monitor_id)
);
