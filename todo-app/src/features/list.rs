//! Lists (projects) that group a user's todos. A todo with no list is in the Inbox.
//!
//! Like todos, every function takes the acting user's id and treats lists that belong to
//! someone else as if they did not exist.

use mssql_tiberius_bridge::Row;
use topcoat::{Result, context::Cx};

use crate::db::{DbError, RowExt, db, id_param};

/// The longest list name, in characters.
pub const MAX_NAME_LEN: usize = 80;
/// The most lists one user may have.
pub const MAX_LISTS: usize = 50;

/// A named group of todos (a row of `dbo.lists`).
#[derive(Debug, Clone)]
pub struct List {
    pub id: u64,

    pub name: String,
}

/// Why a list change was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListError {
    /// The name is blank, too long, or contains control characters.
    Name,
    /// The user already has a list with this name (ignoring case).
    Duplicate,
    /// The user has reached [`MAX_LISTS`].
    TooMany,
    /// The list does not exist or belongs to someone else.
    NotFound,
}

impl ListError {
    /// The message shown on the form.
    pub fn message(self) -> &'static str {
        match self {
            Self::Name => "Enter a list name of up to 80 characters.",
            Self::Duplicate => "You already have a list with that name.",
            Self::TooMany => "You have reached the limit of 50 lists.",
            Self::NotFound => "That list no longer exists.",
        }
    }

    /// A short code carried in the redirect back to the page.
    pub fn code(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Duplicate => "duplicate",
            Self::TooMany => "limit",
            Self::NotFound => "missing",
        }
    }

    /// The inverse of [`ListError::code`].
    pub fn from_code(code: &str) -> Option<Self> {
        [Self::Name, Self::Duplicate, Self::TooMany, Self::NotFound]
            .into_iter()
            .find(|error| error.code() == code)
    }
}

/// Trims a list name and returns it if it is allowed: not blank, at most
/// [`MAX_NAME_LEN`] characters, no control characters (such as newlines).
pub fn clean_name(raw: &str) -> Option<String> {
    let name = raw.trim();
    let valid = !name.is_empty()
        && name.chars().count() <= MAX_NAME_LEN
        && !name.chars().any(char::is_control);
    valid.then(|| name.to_owned())
}

/// Whether `name` is already used by one of `lists`, ignoring case, other than the list
/// `except` (the one being renamed).
fn name_taken(lists: &[List], name: &str, except: Option<u64>) -> bool {
    let wanted = name.to_lowercase();
    lists
        .iter()
        .any(|list| Some(list.id) != except && list.name.to_lowercase() == wanted)
}

impl List {
    fn from_row(row: &Row) -> Result<Self, DbError> {
        Ok(Self {
            id: row.id("id")?,
            name: row.req("name")?,
        })
    }

    /// The user's lists, oldest first.
    pub async fn list(cx: &Cx, user_id: u64) -> Result<Vec<List>> {
        let rows = db(cx)
            .query(
                "SELECT id, name FROM dbo.lists WHERE user_id = @P1 ORDER BY id",
                &[&id_param(user_id)],
            )
            .await?;
        Ok(rows
            .iter()
            .map(Self::from_row)
            .collect::<Result<_, DbError>>()?)
    }

    /// The user's list with this id, or `None` if it doesn't exist or isn't theirs.
    pub async fn find_owned(cx: &Cx, user_id: u64, id: u64) -> Result<Option<List>> {
        let row = db(cx)
            .query_one(
                "SELECT id, name FROM dbo.lists WHERE id = @P1 AND user_id = @P2",
                &[&id_param(id), &id_param(user_id)],
            )
            .await?;
        Ok(row.as_ref().map(Self::from_row).transpose()?)
    }

    /// Creates a list.
    pub async fn add(
        cx: &Cx,
        user_id: u64,
        name: &str,
    ) -> Result<std::result::Result<List, ListError>> {
        let Some(name) = clean_name(name) else {
            return Ok(Err(ListError::Name));
        };
        let existing = List::list(cx, user_id).await?;
        if existing.len() >= MAX_LISTS {
            return Ok(Err(ListError::TooMany));
        }
        if name_taken(&existing, &name, None) {
            return Ok(Err(ListError::Duplicate));
        }
        let rows = db(cx)
            .query(
                "INSERT INTO dbo.lists (user_id, name)                  OUTPUT INSERTED.id, INSERTED.name                  VALUES (@P1, @P2)",
                &[&id_param(user_id), &name.as_str()],
            )
            .await?;
        let list = rows
            .first()
            .map(Self::from_row)
            .unwrap_or_else(|| Err(DbError::Column("id".to_owned())))?;
        Ok(Ok(list))
    }

    /// Renames the user's list.
    pub async fn rename(
        cx: &Cx,
        user_id: u64,
        id: u64,
        name: &str,
    ) -> Result<std::result::Result<(), ListError>> {
        if List::find_owned(cx, user_id, id).await?.is_none() {
            return Ok(Err(ListError::NotFound));
        }
        let Some(name) = clean_name(name) else {
            return Ok(Err(ListError::Name));
        };
        let existing = List::list(cx, user_id).await?;
        if name_taken(&existing, &name, Some(id)) {
            return Ok(Err(ListError::Duplicate));
        }
        db(cx)
            .execute(
                "UPDATE dbo.lists SET name = @P1 WHERE id = @P2 AND user_id = @P3",
                &[&name.as_str(), &id_param(id), &id_param(user_id)],
            )
            .await?;
        Ok(Ok(()))
    }

    /// Deletes the user's list. Its todos are not deleted: they move to the Inbox.
    /// Returns `false`, changing nothing, if the list doesn't exist or isn't theirs.
    pub async fn remove(cx: &Cx, user_id: u64, id: u64) -> Result<bool> {
        if List::find_owned(cx, user_id, id).await?.is_none() {
            return Ok(false);
        }
        // One transaction, so a failure cannot leave todos pointing at a deleted list.
        let mut tx = db(cx).begin().await?;
        tx.execute(
            "UPDATE dbo.todos SET list_id = NULL WHERE list_id = @P1 AND user_id = @P2",
            &[&id_param(id), &id_param(user_id)],
        )
        .await?;
        tx.execute(
            "DELETE FROM dbo.lists WHERE id = @P1 AND user_id = @P2",
            &[&id_param(id), &id_param(user_id)],
        )
        .await?;
        tx.commit().await?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::todo::{Todo, TodoInput};

    const ALICE: u64 = 1;
    const BOB: u64 = 2;

    async fn names(cx: &Cx, user_id: u64) -> Vec<String> {
        List::list(cx, user_id)
            .await
            .unwrap()
            .into_iter()
            .map(|list| list.name)
            .collect()
    }

    #[test]
    fn names_are_trimmed_and_checked() {
        assert_eq!(clean_name("  Work "), Some("Work".to_owned()));
        assert_eq!(clean_name(""), None);
        assert_eq!(clean_name("   "), None);
        assert_eq!(clean_name("two\nlines"), None);
        assert_eq!(clean_name("tab\there"), None);
        assert!(clean_name(&"é".repeat(MAX_NAME_LEN)).is_some());
        assert_eq!(clean_name(&"é".repeat(MAX_NAME_LEN + 1)), None);
    }

    #[test]
    fn error_codes_round_trip() {
        for error in [
            ListError::Name,
            ListError::Duplicate,
            ListError::TooMany,
            ListError::NotFound,
        ] {
            assert_eq!(ListError::from_code(error.code()), Some(error));
        }
        assert_eq!(ListError::from_code("nope"), None);
    }

    #[tokio::test]
    async fn lists_belong_to_their_owner() {
        let cx = crate::db::test_cx().await;
        let work = List::add(&cx, ALICE, "Work").await.unwrap().unwrap();
        List::add(&cx, ALICE, "Home").await.unwrap().unwrap();
        List::add(&cx, BOB, "Bob's").await.unwrap().unwrap();

        assert_eq!(names(&cx, ALICE).await, ["Work", "Home"]);
        assert_eq!(names(&cx, BOB).await, ["Bob's"]);
        assert!(
            List::find_owned(&cx, ALICE, work.id)
                .await
                .unwrap()
                .is_some()
        );
        assert!(List::find_owned(&cx, BOB, work.id).await.unwrap().is_none());
        assert!(
            List::find_owned(&cx, ALICE, work.id + 1000)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn names_must_be_unique_per_user_ignoring_case() {
        let cx = crate::db::test_cx().await;
        List::add(&cx, ALICE, "Work").await.unwrap().unwrap();
        assert_eq!(
            List::add(&cx, ALICE, "work").await.unwrap().err(),
            Some(ListError::Duplicate)
        );
        assert_eq!(
            List::add(&cx, ALICE, " WORK ").await.unwrap().err(),
            Some(ListError::Duplicate)
        );
        // Another user may use the same name.
        assert!(List::add(&cx, BOB, "Work").await.unwrap().is_ok());
        assert_eq!(
            List::add(&cx, ALICE, "  ").await.unwrap().err(),
            Some(ListError::Name)
        );
    }

    #[tokio::test]
    async fn a_user_can_have_at_most_the_limit() {
        let cx = crate::db::test_cx().await;
        for i in 0..MAX_LISTS {
            List::add(&cx, ALICE, &format!("list {i}"))
                .await
                .unwrap()
                .unwrap();
        }
        assert_eq!(
            List::add(&cx, ALICE, "one too many").await.unwrap().err(),
            Some(ListError::TooMany)
        );
        assert!(
            List::add(&cx, BOB, "Bob is unaffected")
                .await
                .unwrap()
                .is_ok()
        );
    }

    #[tokio::test]
    async fn rename_works_for_the_owner_only() {
        let cx = crate::db::test_cx().await;
        let work = List::add(&cx, ALICE, "Work").await.unwrap().unwrap();
        List::add(&cx, ALICE, "Home").await.unwrap().unwrap();

        assert_eq!(
            List::rename(&cx, BOB, work.id, "Hacked").await.unwrap(),
            Err(ListError::NotFound)
        );
        assert_eq!(
            names(&cx, ALICE).await,
            ["Work", "Home"],
            "Bob's rename changed nothing"
        );

        assert_eq!(
            List::rename(&cx, ALICE, work.id, "Job").await.unwrap(),
            Ok(())
        );
        assert_eq!(names(&cx, ALICE).await, ["Job", "Home"]);
        // Renaming to its own name (or just changing the case) is fine; taking another's is not.
        assert_eq!(
            List::rename(&cx, ALICE, work.id, "JOB").await.unwrap(),
            Ok(())
        );
        assert_eq!(
            List::rename(&cx, ALICE, work.id, "home").await.unwrap(),
            Err(ListError::Duplicate)
        );
        assert_eq!(
            List::rename(&cx, ALICE, work.id, "").await.unwrap(),
            Err(ListError::Name)
        );
        assert_eq!(
            List::rename(&cx, ALICE, work.id + 1000, "x").await.unwrap(),
            Err(ListError::NotFound)
        );
    }

    async fn add_in(cx: &Cx, title: &str, list: Option<u64>) {
        let mut input = TodoInput::parse(title, "", "medium", "").unwrap();
        input.list_id = list;
        assert!(Todo::add(cx, ALICE, &input).await.unwrap());
    }

    #[tokio::test]
    async fn deleting_a_list_moves_its_todos_to_the_inbox() {
        let cx = crate::db::test_cx().await;
        let work = List::add(&cx, ALICE, "Work").await.unwrap().unwrap();
        let home = List::add(&cx, ALICE, "Home").await.unwrap().unwrap();
        add_in(&cx, "in work", Some(work.id)).await;
        add_in(&cx, "also work", Some(work.id)).await;
        add_in(&cx, "in home", Some(home.id)).await;
        add_in(&cx, "in inbox", None).await;

        assert!(List::remove(&cx, ALICE, work.id).await.unwrap());
        assert_eq!(names(&cx, ALICE).await, ["Home"]);

        let todos = Todo::list(&cx, ALICE).await.unwrap();
        assert_eq!(todos.len(), 4, "no todo was deleted");
        let list_of = |title: &str| todos.iter().find(|t| t.title == title).unwrap().list_id;
        assert_eq!(list_of("in work"), None);
        assert_eq!(list_of("also work"), None);
        assert_eq!(
            list_of("in home"),
            Some(home.id),
            "other lists are untouched"
        );
        assert_eq!(list_of("in inbox"), None);
    }

    #[tokio::test]
    async fn another_user_cannot_delete_a_list() {
        let cx = crate::db::test_cx().await;
        let work = List::add(&cx, ALICE, "Work").await.unwrap().unwrap();
        add_in(&cx, "keep me here", Some(work.id)).await;

        assert!(!List::remove(&cx, BOB, work.id).await.unwrap());
        assert!(!List::remove(&cx, ALICE, work.id + 1000).await.unwrap());
        assert_eq!(names(&cx, ALICE).await, ["Work"]);
        let todos = Todo::list(&cx, ALICE).await.unwrap();
        assert_eq!(
            todos[0].list_id,
            Some(work.id),
            "the todo stayed in its list"
        );
    }
}
