ALTER TABLE "todos" ADD COLUMN "notes" TEXT;
-- #[toasty::breakpoint]
ALTER TABLE "todos" ADD COLUMN "due_on" TEXT;
-- #[toasty::breakpoint]
-- Hand-edited: SQLite cannot add a NOT NULL column without a default to a table that
-- already has rows, so existing todos get medium priority.
ALTER TABLE "todos" ADD COLUMN "priority" TEXT NOT NULL DEFAULT 'medium';
