//! The database connection.

use toasty::Db;
use topcoat::context::{Cx, context};

/// Every migration in `toasty/migrations`, compiled into the executable, so a deployment
/// needs no loose migration files.
static MIGRATIONS: toasty::migration::MigrationSet = toasty::embed_migrations!();

/// Opens the SQLite database at `url` (for example `sqlite:data.db`, created when it is
/// first opened) with every model in the application.
///
/// The tables come from migrations, which the app applies at startup (see [`migrate`]).
/// After changing a model, generate a migration with
/// `cargo run -- toasty migration generate` and rebuild.
pub async fn connect(url: &str) -> toasty::Result<Db> {
    Db::builder()
        .models(toasty::models!(crate::*))
        .connect(url)
        .await
}

/// Applies the migrations this database hasn't seen yet and returns how many that was.
/// Running it again changes nothing.
pub async fn migrate(db: &Db) -> toasty::Result<usize> {
    Ok(MIGRATIONS.apply(db).await?.applied())
}

/// A context backed by a fresh in-memory database with every table created, plus a new
/// rate limiter, for tests of code that takes a `&Cx`.
#[cfg(test)]
pub async fn test_cx() -> Cx {
    let db = Db::builder()
        .models(toasty::models!(crate::*))
        .connect("sqlite::memory:")
        .await
        .expect("in-memory database opens");
    db.push_schema().await.expect("tables are created");
    Cx::new()
        .with(db)
        .with(crate::features::throttle::Throttle::new())
        .with(crate::config::Registration::Open)
}

/// Clones the database handle stored in the context. The clone uses the same
/// connection pool, so it is cheap to create.
pub fn db(cx: &Cx) -> Db {
    context::<Db>(cx).clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::{
        list::List,
        todo::{Priority, Todo, TodoInput},
        user::{SessionRecord, User},
    };

    /// A database with no tables, as a new deployment starts.
    async fn empty() -> Db {
        Db::builder()
            .models(toasty::models!(crate::*))
            .connect("sqlite::memory:")
            .await
            .expect("in-memory database opens")
    }

    /// If a model changes without a migration being generated, this fails: it builds the
    /// schema only from the embedded migrations (as production does) and then uses every
    /// table and every column through the real data layer.
    #[tokio::test]
    async fn the_embedded_migrations_build_a_schema_the_models_can_use() {
        let db = empty().await;
        let applied = migrate(&db).await.expect("migrations apply");
        assert!(applied >= 4, "applied {applied}");

        let cx = Cx::new().with(db.clone());
        let mut handle = db.clone();

        let user = toasty::create!(User {
            email: "someone@example.com",
            password_hash: "hash",
        })
        .exec(&mut handle)
        .await
        .expect("users table");
        toasty::create!(SessionRecord {
            token_hash: vec![9u8; 32],
            user_id: user.id,
            expires_at: 123i64,
        })
        .exec(&mut handle)
        .await
        .expect("session_records table");

        // The password_resets table: a link can be made and then found again.
        let link = crate::features::reset::issue(&mut handle, user.id, 0)
            .await
            .expect("password_resets table");
        assert!(
            crate::features::reset::find_valid(&mut handle, &link, 1)
                .await
                .unwrap()
                .is_some()
        );

        let list = List::add(&cx, user.id, "Work")
            .await
            .unwrap()
            .expect("lists table");
        let mut input = TodoInput::parse("Write", "some notes", "high", "2030-05-06").unwrap();
        input.list_id = Some(list.id);
        assert!(Todo::add(&cx, user.id, &input).await.expect("todos table"));

        let todos = Todo::list(&cx, user.id).await.unwrap();
        assert_eq!(todos.len(), 1);
        let todo = &todos[0];
        assert_eq!(todo.title, "Write");
        assert_eq!(todo.notes.as_deref(), Some("some notes"));
        assert_eq!(todo.priority(), Priority::High);
        assert_eq!(
            todo.due().map(|d| d.to_iso()).as_deref(),
            Some("2030-05-06")
        );
        assert_eq!(todo.list_id, Some(list.id));
    }

    #[tokio::test]
    async fn migrating_twice_changes_nothing() {
        let db = empty().await;
        assert!(migrate(&db).await.unwrap() > 0);
        assert_eq!(migrate(&db).await.unwrap(), 0);
    }
}
