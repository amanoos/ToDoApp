-- The initial schema. db.rs runs this batch once, in a transaction, and records it in
-- dbo.schema_migrations. Never edit a released migration: add a new file and list it in
-- src/db.rs instead.
--
-- Notes on the choices:
--  * Keys are BIGINT IDENTITY; the app reads them as u64.
--  * Text is NVARCHAR (UTF-16). The lengths allow for characters outside the Basic
--    Multilingual Plane, which take two code units, so they are twice the app's limits
--    (title 200, notes 2000, list name 80 characters).
--  * Timestamps are BIGINT seconds since the Unix epoch, as the app computes them.
--  * There are no foreign keys: the app deletes an account's rows itself, in one transaction
--    (features/account.rs), and checks ownership on every query.

CREATE TABLE dbo.users (
    id            BIGINT IDENTITY(1, 1) NOT NULL CONSTRAINT pk_users PRIMARY KEY,
    email         NVARCHAR(320)         NOT NULL,
    password_hash NVARCHAR(255)         NOT NULL
);
CREATE UNIQUE INDEX ux_users_email ON dbo.users (email);

CREATE TABLE dbo.session_records (
    id         BIGINT IDENTITY(1, 1) NOT NULL CONSTRAINT pk_session_records PRIMARY KEY,
    token_hash VARBINARY(64)         NOT NULL,
    user_id    BIGINT                NOT NULL,
    expires_at BIGINT                NOT NULL
);
CREATE UNIQUE INDEX ux_session_records_token_hash ON dbo.session_records (token_hash);
CREATE INDEX ix_session_records_user_id ON dbo.session_records (user_id);

CREATE TABLE dbo.password_resets (
    id         BIGINT IDENTITY(1, 1) NOT NULL CONSTRAINT pk_password_resets PRIMARY KEY,
    user_id    BIGINT                NOT NULL,
    token_hash VARBINARY(64)         NOT NULL,
    expires_at BIGINT                NOT NULL
);
CREATE UNIQUE INDEX ux_password_resets_token_hash ON dbo.password_resets (token_hash);
CREATE INDEX ix_password_resets_user_id ON dbo.password_resets (user_id);

CREATE TABLE dbo.lists (
    id      BIGINT IDENTITY(1, 1) NOT NULL CONSTRAINT pk_lists PRIMARY KEY,
    user_id BIGINT                NOT NULL,
    name    NVARCHAR(160)         NOT NULL
);
CREATE INDEX ix_lists_user_id ON dbo.lists (user_id);

CREATE TABLE dbo.todos (
    id       BIGINT IDENTITY(1, 1) NOT NULL CONSTRAINT pk_todos PRIMARY KEY,
    user_id  BIGINT                NOT NULL,
    title    NVARCHAR(400)         NOT NULL,
    done     BIT                   NOT NULL CONSTRAINT df_todos_done DEFAULT 0,
    notes    NVARCHAR(4000)        NULL,
    priority NVARCHAR(10)          NOT NULL CONSTRAINT df_todos_priority DEFAULT N'medium',
    due_on   NVARCHAR(10)          NULL,
    list_id  BIGINT                NULL
);
CREATE INDEX ix_todos_user_id ON dbo.todos (user_id);
CREATE INDEX ix_todos_list_id ON dbo.todos (list_id);
