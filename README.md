<div align="center">

# Is it <span style="color: lightgreen">online?</span>

A self-hosted web app for monitoring your services

</div>

## Installation

### Docker Compose

The easist way to host this yourself is with docker compose.

> [!NOTE]  
> As this is still in development, no docker images have been published yet. You will have to build the image youself.

**Example `Docker-Compose.yml`**

```yml
version: '3.1'
services:
  isitonline:
    image: isitonline
    environment:
      POSTGRES_PASSWORD: example
    ports:
      - 8080:8080
    volumes:
      - db_data:/var/lib/mysql

volumes:
  db_data:
```

### Windows

## Development

**Database**

1. Install the [sqlx cli](https://github.com/launchbadge/sqlx) cli with `cargo install sqlx-cli`
2. Create a `DATABASE_URL` enviroment variable with the value `sqlite:db/data.db`
3. Create the database with `sqlx database create`
3. Set it up with the tables using `sqlx migrate run`

**Tailwind CSS**

1. Install the [tailwind cli](https://tailwindcss.com/docs/installation) cli with `npm install -g tailwindcss` or from the [GitHub releases](https://github.com/tailwindlabs/tailwindcss/releases) if you don't want to use Node.js
2. Start the cli with `tailwindcss -i ./src/base.css -o ./static/style.css --watch`

**Running**

After the setup you can run the app with `cargo r` and it should appear at http://127.0.0.1:8080. You could also use the dockerfile for development if you'd rather not install extra cli's or just prefer docker, however the included image doesn't cache dependencies yet so it's very slow.

**Useful Tools**

- [DB Browser for SQLite](https://sqlitebrowser.org/) has been really useful durning development for viewing the database in an easy to use gui, you can download it from their [GitHub releases](https://github.com/sqlitebrowser/sqlitebrowser/releases)

## Credits

A lot of the design is based of [this design by Fireart Studio](https://dribbble.com/shots/16937445-Uptime-Monitoring-Service-Dashboard/attachments/12008723?mode=media).
