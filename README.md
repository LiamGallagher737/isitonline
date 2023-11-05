<div align="center">

# Is it <span style="color: lightgreen">online?</span>

A self-hosted web app for monitoring your services

</div>

## Installation

### Docker

### Windows

## Development

Steps to setup a development enviroment

1. Install the [sqlx cli](https://github.com/launchbadge/sqlx) cli with `cargo install sqlx-cli`
2. Create a `DATABASE_URL` enviroment variable with the value `sqlite:db/data.db`
3. Create the database with `sqlx database create`
3. Set it up with the tables using `sqlx migrate run`
4. Run the app with `cargo r` and it should appear at http://127.0.0.1:8080