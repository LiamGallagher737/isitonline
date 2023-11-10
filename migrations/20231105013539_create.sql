CREATE TABLE monitors(
    id INTEGER PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    target TEXT NOT NULL,
    cron TEXT NOT NULL
);

CREATE TABLE changes(
    timestamp TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    success BOOLEAN NOT NULL,
    http_code INTEGER,
    monitor_id INTEGER NOT NULL,
    FOREIGN KEY (monitor_id)
       REFERENCES monitors (id)
);

CREATE TABLE response_times(
    timestamp TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    time INTEGER NOT NULL,
    monitor_id INTEGER NOT NULL,
    FOREIGN KEY (monitor_id)
       REFERENCES monitors (id)
);

CREATE TABLE monitors_summary (
    monitor_id INTEGER PRIMARY KEY,
    timestamp TIMESTAMP NOT NULL,
    name TEXT NOT NULL,
    success BOOLEAN,
    http_code INTEGER,
    response_time INTEGER,
    FOREIGN KEY (monitor_id)
       REFERENCES monitors (id)
);

CREATE TRIGGER insert_monitor_summary
    AFTER INSERT ON monitors
BEGIN
    INSERT INTO monitors_summary (monitor_id, timestamp, name)
    VALUES (new.id, CURRENT_TIMESTAMP, new.name);
END;

CREATE TRIGGER update_summary_on_changes
    AFTER INSERT ON changes
    FOR EACH ROW
BEGIN
    UPDATE monitors_summary
    SET
        success = NEW.success,
        http_code = NEW.http_code
    WHERE monitor_id = NEW.monitor_id;
END;

CREATE TRIGGER update_summary_on_response_times
    AFTER INSERT ON response_times
    FOR EACH ROW
BEGIN
    UPDATE monitors_summary
    SET
        response_time = NEW.time,
        timestamp = NEW.timestamp
    WHERE monitor_id = NEW.monitor_id;
END;
