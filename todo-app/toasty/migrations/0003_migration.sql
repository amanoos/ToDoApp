ALTER TABLE "todos" ADD COLUMN "list_id" INTEGER;
-- #[toasty::breakpoint]
CREATE TABLE "lists" (
    "id" INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
    "user_id" INTEGER NOT NULL,
    "name" TEXT NOT NULL
);
-- #[toasty::breakpoint]
CREATE INDEX "index_lists_by_user_id" ON "lists" ("user_id");
