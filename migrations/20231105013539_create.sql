DROP TABLE IF EXISTS monitors;

CREATE TABLE monitors(
    name TEXT NOT NULL,
    target TEXT NOT NULL,
    cron TEXT NOT NULL
);
