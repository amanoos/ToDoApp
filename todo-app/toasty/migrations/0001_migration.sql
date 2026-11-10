ALTER TABLE "todos" ADD COLUMN "user_id" INTEGER NOT NULL;
-- #[toasty::breakpoint]
CREATE INDEX "index_todos_by_user_id" ON "todos" ("user_id");
-- #[toasty::breakpoint]
CREATE TABLE "session_records" (
    "id" INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
    "token_hash" BLOB NOT NULL,
    "user_id" INTEGER NOT NULL,
    "expires_at" BIGINT NOT NULL
);
-- #[toasty::breakpoint]
CREATE UNIQUE INDEX "index_session_records_by_token_hash" ON "session_records" ("token_hash");
-- #[toasty::breakpoint]
CREATE INDEX "index_session_records_by_user_id" ON "session_records" ("user_id");
-- #[toasty::breakpoint]
CREATE TABLE "users" (
    "id" INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
    "email" TEXT NOT NULL,
    "password_hash" TEXT NOT NULL
);
-- #[toasty::breakpoint]
CREATE UNIQUE INDEX "index_users_by_email" ON "users" ("email");
