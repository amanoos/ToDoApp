//! Deleting an account and everything it owns.

use topcoat::{Result, context::Cx};

use super::{
    list::List,
    reset::PasswordReset,
    todo::Todo,
    user::{self, PasswordError, SessionRecord, User, normalize_email},
};
use crate::db::db;

/// Why a deletion was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteError {
    /// The password given is wrong (or the account no longer exists).
    WrongPassword,
    /// The typed email does not match the account's.
    WrongConfirmation,
    /// Too many wrong password answers recently.
    Throttled,
}

impl DeleteError {
    /// The message shown on the form.
    pub fn message(self) -> &'static str {
        match self {
            Self::WrongPassword => "The password is not correct. Nothing was deleted.",
            Self::WrongConfirmation => {
                "The email you typed does not match this account. Nothing was deleted."
            }
            Self::Throttled => "Too many attempts. Please wait a few minutes and try again.",
        }
    }

    /// A short code carried in the redirect back to the form.
    pub fn code(self) -> &'static str {
        match self {
            Self::WrongPassword => "delete-password",
            Self::WrongConfirmation => "delete-confirm",
            Self::Throttled => "delete-throttled",
        }
    }

    /// The inverse of [`DeleteError::code`].
    pub fn from_code(code: &str) -> Option<Self> {
        [
            Self::WrongPassword,
            Self::WrongConfirmation,
            Self::Throttled,
        ]
        .into_iter()
        .find(|error| error.code() == code)
    }
}

/// What a deletion removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Purged {
    /// Whether the account itself existed (and so was deleted).
    pub account: bool,
    pub todos: usize,
    pub lists: usize,
    pub sessions: usize,
}

/// What [`purge_user`] would remove for this user, without removing anything.
pub async fn owned_by(db: &mut toasty::Db, user_id: u64) -> Result<Purged> {
    Ok(Purged {
        account: User::filter_by_id(user_id)
            .first()
            .exec(&mut *db)
            .await?
            .is_some(),
        todos: Todo::filter_by_user_id(user_id).exec(&mut *db).await?.len(),
        lists: List::filter_by_user_id(user_id).exec(&mut *db).await?.len(),
        sessions: SessionRecord::filter_by_user_id(user_id)
            .exec(&mut *db)
            .await?
            .len(),
    })
}

/// Deletes the user and everything they own: sessions, todos, lists, then the account, all
/// in one transaction, so it either happens completely or not at all (a failure part-way
/// rolls everything back instead of leaving orphaned data). Deleting a user that does not
/// exist removes nothing and is not an error.
pub async fn purge_user(db: &mut toasty::Db, user_id: u64) -> Result<Purged> {
    let mut tx = db.transaction().await?;

    let sessions = SessionRecord::filter_by_user_id(user_id)
        .exec(&mut tx)
        .await?
        .len();
    SessionRecord::filter_by_user_id(user_id)
        .delete()
        .exec(&mut tx)
        .await?;

    let todos = Todo::filter_by_user_id(user_id).exec(&mut tx).await?.len();
    Todo::filter_by_user_id(user_id)
        .delete()
        .exec(&mut tx)
        .await?;

    let lists = List::filter_by_user_id(user_id).exec(&mut tx).await?.len();
    List::filter_by_user_id(user_id)
        .delete()
        .exec(&mut tx)
        .await?;

    // A pending reset link must not outlive the account it was for.
    PasswordReset::filter_by_user_id(user_id)
        .delete()
        .exec(&mut tx)
        .await?;

    let account = User::filter_by_id(user_id)
        .first()
        .exec(&mut tx)
        .await?
        .is_some();
    User::filter_by_id(user_id).delete().exec(&mut tx).await?;

    tx.commit().await?;
    Ok(Purged {
        account,
        todos,
        lists,
        sessions,
    })
}

/// Deletes the user's own account, if they prove it is them and that they mean it: the
/// current password, and the account's email typed out. This cannot be undone.
///
/// The password check shares its rate limits with changing the password
/// ([`user::verify_current_password`]), so someone holding a stolen session cannot use this
/// form to guess the password. A mistyped confirmation is a form mistake and is reported
/// before the password is even checked, so it costs no hashing and counts as no guess.
pub async fn delete_account(
    cx: &Cx,
    user_id: u64,
    password: &str,
    confirm_email: &str,
) -> Result<std::result::Result<Purged, DeleteError>> {
    let Some(account) = User::filter_by_id(user_id)
        .first()
        .exec(&mut db(cx))
        .await?
    else {
        return Ok(Err(DeleteError::WrongPassword));
    };
    if normalize_email(confirm_email).as_deref() != Some(account.email.as_str()) {
        return Ok(Err(DeleteError::WrongConfirmation));
    }

    match user::verify_current_password(cx, user_id, password).await? {
        Ok(_) => {}
        Err(PasswordError::Throttled) => return Ok(Err(DeleteError::Throttled)),
        Err(_) => return Ok(Err(DeleteError::WrongPassword)),
    }

    let mut handle = db(cx);
    Ok(Ok(purge_user(&mut handle, user_id).await?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::{
        todo::TodoInput,
        user::{create_account, verify_and_set},
    };

    const PASSWORD: &str = "a fine long password";
    const NEW_PASSWORD: &str = "some brand new password 1";

    async fn account(cx: &Cx, email: &str) -> User {
        let mut handle = db(cx);
        create_account(&mut handle, email, PASSWORD)
            .await
            .unwrap()
            .expect("account created")
    }

    /// Gives the user two todos, two lists and a session.
    async fn give_data(cx: &Cx, user: &User) {
        for name in ["Work", "Home"] {
            List::add(cx, user.id, name).await.unwrap().unwrap();
        }
        for title in ["one", "two"] {
            let input = TodoInput::parse(title, "", "medium", "").unwrap();
            assert!(Todo::add(cx, user.id, &input).await.unwrap());
        }
        toasty::create!(SessionRecord {
            token_hash: vec![user.id as u8; 32],
            user_id: user.id,
            expires_at: 9_999_999_999i64,
        })
        .exec(&mut db(cx))
        .await
        .unwrap();
    }

    /// (todos, lists, sessions, accounts) belonging to the user.
    async fn owned(cx: &Cx, user_id: u64) -> (usize, usize, usize, usize) {
        let mut handle = db(cx);
        (
            Todo::filter_by_user_id(user_id)
                .exec(&mut handle)
                .await
                .unwrap()
                .len(),
            List::filter_by_user_id(user_id)
                .exec(&mut handle)
                .await
                .unwrap()
                .len(),
            SessionRecord::filter_by_user_id(user_id)
                .exec(&mut handle)
                .await
                .unwrap()
                .len(),
            User::filter_by_id(user_id)
                .exec(&mut handle)
                .await
                .unwrap()
                .len(),
        )
    }

    async fn delete(
        cx: &Cx,
        user: &User,
        password: &str,
        confirm: &str,
    ) -> std::result::Result<Purged, DeleteError> {
        delete_account(cx, user.id, password, confirm)
            .await
            .expect("no internal error")
    }

    #[tokio::test]
    async fn purging_removes_everything_the_user_owns_and_nothing_else() {
        let cx = crate::db::test_cx().await;
        let alice = account(&cx, "alice@example.com").await;
        let bob = account(&cx, "bob@example.com").await;
        give_data(&cx, &alice).await;
        give_data(&cx, &bob).await;
        assert_eq!(owned(&cx, alice.id).await, (2, 2, 1, 1));

        let purged = purge_user(&mut db(&cx), alice.id).await.unwrap();
        assert_eq!(
            purged,
            Purged {
                account: true,
                todos: 2,
                lists: 2,
                sessions: 1
            }
        );
        assert_eq!(
            owned(&cx, alice.id).await,
            (0, 0, 0, 0),
            "nothing of hers is left"
        );
        assert_eq!(owned(&cx, bob.id).await, (2, 2, 1, 1), "Bob is untouched");
    }

    #[tokio::test]
    async fn owned_by_reports_what_a_purge_would_remove_without_removing_it() {
        let cx = crate::db::test_cx().await;
        let alice = account(&cx, "alice@example.com").await;
        give_data(&cx, &alice).await;

        let mut handle = db(&cx);
        let preview = owned_by(&mut handle, alice.id).await.unwrap();
        assert_eq!(
            preview,
            Purged {
                account: true,
                todos: 2,
                lists: 2,
                sessions: 1
            }
        );
        assert_eq!(
            owned(&cx, alice.id).await,
            (2, 2, 1, 1),
            "nothing was removed"
        );
        assert_eq!(
            purge_user(&mut handle, alice.id).await.unwrap(),
            preview,
            "the preview matches what the purge then removes"
        );
        assert_eq!(
            owned_by(&mut handle, alice.id).await.unwrap(),
            Purged::default()
        );
    }

    #[tokio::test]
    async fn purging_also_removes_a_pending_reset_link_and_only_that_users() {
        let cx = crate::db::test_cx().await;
        let alice = account(&cx, "alice@example.com").await;
        let bob = account(&cx, "bob@example.com").await;
        let alices_link = crate::features::reset::issue(&mut db(&cx), alice.id, 1_000)
            .await
            .unwrap();
        let bobs_link = crate::features::reset::issue(&mut db(&cx), bob.id, 1_000)
            .await
            .unwrap();

        purge_user(&mut db(&cx), alice.id).await.unwrap();
        let mut handle = db(&cx);
        assert!(
            crate::features::reset::find_valid(&mut handle, &alices_link, 1_000)
                .await
                .unwrap()
                .is_none(),
            "her link died with her account"
        );
        assert!(
            crate::features::reset::find_valid(&mut handle, &bobs_link, 1_000)
                .await
                .unwrap()
                .is_some(),
            "Bob's link is untouched"
        );
    }

    #[tokio::test]
    async fn purging_a_missing_user_removes_nothing_and_is_not_an_error() {
        let cx = crate::db::test_cx().await;
        let bob = account(&cx, "bob@example.com").await;
        give_data(&cx, &bob).await;
        let purged = purge_user(&mut db(&cx), 987_654).await.unwrap();
        assert_eq!(purged, Purged::default());
        assert_eq!(owned(&cx, bob.id).await, (2, 2, 1, 1));
        // Purging twice is harmless too.
        purge_user(&mut db(&cx), bob.id).await.unwrap();
        assert_eq!(
            purge_user(&mut db(&cx), bob.id).await.unwrap(),
            Purged::default()
        );
    }

    #[tokio::test]
    async fn the_right_password_and_email_delete_the_account() {
        let cx = crate::db::test_cx().await;
        let alice = account(&cx, "alice@example.com").await;
        give_data(&cx, &alice).await;

        // The email is matched like at sign-up: case and surrounding spaces don't matter.
        let purged = delete(&cx, &alice, PASSWORD, "  ALICE@example.com ")
            .await
            .unwrap();
        assert_eq!(
            purged,
            Purged {
                account: true,
                todos: 2,
                lists: 2,
                sessions: 1
            }
        );
        assert_eq!(owned(&cx, alice.id).await, (0, 0, 0, 0));
    }

    #[tokio::test]
    async fn a_wrong_password_deletes_nothing() {
        let cx = crate::db::test_cx().await;
        let alice = account(&cx, "alice@example.com").await;
        give_data(&cx, &alice).await;
        assert_eq!(
            delete(&cx, &alice, "not the password", "alice@example.com").await,
            Err(DeleteError::WrongPassword)
        );
        assert_eq!(
            delete(&cx, &alice, "", "alice@example.com").await,
            Err(DeleteError::WrongPassword)
        );
        assert_eq!(owned(&cx, alice.id).await, (2, 2, 1, 1));
    }

    #[tokio::test]
    async fn a_mistyped_confirmation_deletes_nothing_and_does_not_count_as_a_guess() {
        let cx = crate::db::test_cx().await;
        let alice = account(&cx, "alice@example.com").await;
        give_data(&cx, &alice).await;
        for wrong in [
            "",
            "alice",
            "alice@example.org",
            "bob@example.com",
            "alice@example.com extra",
        ] {
            assert_eq!(
                delete(&cx, &alice, PASSWORD, wrong).await,
                Err(DeleteError::WrongConfirmation),
                "{wrong:?}"
            );
        }
        // Far more than the limit of 5, yet it is still possible to delete properly.
        for _ in 0..8 {
            assert_eq!(
                delete(&cx, &alice, PASSWORD, "typo").await,
                Err(DeleteError::WrongConfirmation)
            );
        }
        assert_eq!(owned(&cx, alice.id).await, (2, 2, 1, 1));
        assert!(
            delete(&cx, &alice, PASSWORD, "alice@example.com")
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn another_accounts_email_cannot_be_used_to_confirm() {
        let cx = crate::db::test_cx().await;
        let alice = account(&cx, "alice@example.com").await;
        let _bob = account(&cx, "bob@example.com").await;
        assert_eq!(
            delete(&cx, &alice, PASSWORD, "bob@example.com").await,
            Err(DeleteError::WrongConfirmation)
        );
    }

    #[tokio::test]
    async fn guessing_the_password_here_is_rate_limited_even_for_the_right_one() {
        let cx = crate::db::test_cx().await;
        let alice = account(&cx, "alice@example.com").await;
        give_data(&cx, &alice).await;
        for attempt in 1..=5 {
            assert_eq!(
                delete(&cx, &alice, "wrong guess", "alice@example.com").await,
                Err(DeleteError::WrongPassword),
                "attempt {attempt}"
            );
        }
        assert_eq!(
            delete(&cx, &alice, PASSWORD, "alice@example.com").await,
            Err(DeleteError::Throttled)
        );
        assert_eq!(
            owned(&cx, alice.id).await,
            (2, 2, 1, 1),
            "blocked before anything was deleted"
        );
    }

    #[tokio::test]
    async fn the_limit_is_shared_with_changing_the_password_so_guesses_cannot_be_doubled() {
        let cx = crate::db::test_cx().await;
        let alice = account(&cx, "alice@example.com").await;
        for _ in 0..3 {
            let _ = delete(&cx, &alice, "wrong guess", "alice@example.com").await;
        }
        for _ in 0..2 {
            let _ = verify_and_set(&cx, alice.id, "wrong guess", NEW_PASSWORD).await;
        }
        // Five wrong answers in total, split across the two forms: now both are blocked.
        assert_eq!(
            delete(&cx, &alice, PASSWORD, "alice@example.com").await,
            Err(DeleteError::Throttled)
        );
        assert_eq!(
            verify_and_set(&cx, alice.id, PASSWORD, NEW_PASSWORD)
                .await
                .unwrap(),
            Err(PasswordError::Throttled)
        );
    }

    #[tokio::test]
    async fn the_limit_is_per_account() {
        let cx = crate::db::test_cx().await;
        let alice = account(&cx, "alice@example.com").await;
        let bob = account(&cx, "bob@example.com").await;
        for _ in 0..5 {
            let _ = delete(&cx, &alice, "wrong guess", "alice@example.com").await;
        }
        assert_eq!(
            delete(&cx, &alice, PASSWORD, "alice@example.com").await,
            Err(DeleteError::Throttled)
        );
        assert!(
            delete(&cx, &bob, PASSWORD, "bob@example.com").await.is_ok(),
            "Bob has his own count"
        );
    }

    #[tokio::test]
    async fn an_account_that_no_longer_exists_just_looks_like_a_wrong_password() {
        let cx = crate::db::test_cx().await;
        assert_eq!(
            delete_account(&cx, 987_654, PASSWORD, "x@example.com")
                .await
                .unwrap(),
            Err(DeleteError::WrongPassword)
        );
    }

    #[tokio::test]
    async fn a_deleted_email_can_start_over_with_a_clean_slate() {
        let cx = crate::db::test_cx().await;
        let old = account(&cx, "alice@example.com").await;
        give_data(&cx, &old).await;
        delete(&cx, &old, PASSWORD, "alice@example.com")
            .await
            .unwrap();

        let fresh = account(&cx, "alice@example.com").await;
        assert_ne!(fresh.id, old.id, "a new account, not the old one");
        assert_eq!(
            owned(&cx, fresh.id).await,
            (0, 0, 0, 1),
            "none of the old data came back"
        );
        assert_eq!(owned(&cx, old.id).await, (0, 0, 0, 0));
    }

    /// A database built from the real migrations except that the `lists` table is never
    /// created, so a purge fails at the lists step, *after* it has already deleted the
    /// user's sessions and todos. That is the half-done state a transaction must undo.
    async fn database_without_lists() -> toasty::Db {
        use toasty::migration::{MigrationFile, MigrationSet};

        let sql = [
            include_str!("../../toasty/migrations/0000_migration.sql"),
            include_str!("../../toasty/migrations/0001_migration.sql"),
            include_str!("../../toasty/migrations/0002_migration.sql"),
            include_str!("../../toasty/migrations/0003_migration.sql"),
            include_str!("../../toasty/migrations/0004_migration.sql"),
        ];
        // Keep only 0003's first statement (the `todos.list_id` column), dropping `lists`.
        let without_lists: &'static str = Box::leak(
            sql[3]
                .split("-- #[toasty::breakpoint]")
                .next()
                .unwrap()
                .to_owned()
                .into_boxed_str(),
        );
        let files: &'static [MigrationFile] = Box::leak(Box::new([
            MigrationFile::new(1, "0000_migration.sql", sql[0]),
            MigrationFile::new(2, "0001_migration.sql", sql[1]),
            MigrationFile::new(3, "0002_migration.sql", sql[2]),
            MigrationFile::new(4, "0003_migration.sql", without_lists),
            MigrationFile::new(5, "0004_migration.sql", sql[4]),
        ]));

        let db = toasty::Db::builder()
            .models(toasty::models!(crate::*))
            .connect("sqlite::memory:")
            .await
            .unwrap();
        MigrationSet::new(files).apply(&db).await.unwrap();
        db
    }

    #[tokio::test]
    async fn a_failure_part_way_through_a_purge_rolls_everything_back() {
        let db = database_without_lists().await;
        let cx = Cx::new()
            .with(db.clone())
            .with(crate::features::throttle::Throttle::new())
            .with(crate::config::Registration::Open);
        let alice = account(&cx, "alice@example.com").await;
        let input = TodoInput::parse("keep me", "", "medium", "").unwrap();
        assert!(Todo::add(&cx, alice.id, &input).await.unwrap());
        toasty::create!(SessionRecord {
            token_hash: vec![7u8; 32],
            user_id: alice.id,
            expires_at: 9_999_999_999i64,
        })
        .exec(&mut db.clone())
        .await
        .unwrap();

        // The purge deletes sessions, then todos, then fails on the missing `lists` table.
        let outcome = purge_user(&mut db.clone(), alice.id).await;
        assert!(outcome.is_err(), "the purge must fail here");

        // Nothing was removed, including what had already been deleted inside the transaction.
        let mut handle = db.clone();
        assert_eq!(
            Todo::filter_by_user_id(alice.id)
                .exec(&mut handle)
                .await
                .unwrap()
                .len(),
            1,
            "the todo is back"
        );
        assert_eq!(
            SessionRecord::filter_by_user_id(alice.id)
                .exec(&mut handle)
                .await
                .unwrap()
                .len(),
            1,
            "the session is back"
        );
        assert_eq!(
            User::filter_by_id(alice.id)
                .exec(&mut handle)
                .await
                .unwrap()
                .len(),
            1,
            "the account is still there"
        );
    }

    #[test]
    fn delete_error_codes_round_trip() {
        for error in [
            DeleteError::WrongPassword,
            DeleteError::WrongConfirmation,
            DeleteError::Throttled,
        ] {
            assert_eq!(DeleteError::from_code(error.code()), Some(error));
            assert!(!error.message().is_empty());
        }
        assert_eq!(DeleteError::from_code("nope"), None);
    }
}
