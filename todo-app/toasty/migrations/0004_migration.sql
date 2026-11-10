CREATE TABLE "password_resets" (
    "id" INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
    "user_id" INTEGER NOT NULL,
    "token_hash" BLOB NOT NULL,
    "expires_at" BIGINT NOT NULL
);
-- #[toasty::breakpoint]
CREATE INDEX "index_password_resets_by_user_id" ON "password_resets" ("user_id");
-- #[toasty::breakpoint]
CREATE UNIQUE INDEX "index_password_resets_by_token_hash" ON "password_resets" ("token_hash");
