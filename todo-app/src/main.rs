// The SQL Server driver's futures nest deeply, and proving each page's future is `Send`
// (which the server requires) needs more than the default depth.
#![recursion_limit = "512"]

mod app;
mod components;
mod config;
mod db;
mod features;
mod healthcheck;
mod security;

use std::{
    io::{IsTerminal, stdin},
    process::exit,
};

/// `todo-app create-user <email>`: creates an account directly in the database, which is how
/// the first account is made on a site with sign-ups closed. The password is read from
/// standard input, never from the command line, where it would show up in process listings
/// and shell history. Returns the process exit code.
async fn create_user(db: &db::Db, mut args: impl Iterator<Item = String>) -> i32 {
    args.next(); // the subcommand's own name
    let (Some(email), None) = (args.next(), args.next()) else {
        eprintln!(
            "usage: todo-app create-user <email>   (the password is read from standard input)"
        );
        return 2;
    };

    if stdin().is_terminal() {
        eprintln!(
            "Password (12 to 128 characters; it will be visible, so pipe it in if that matters):"
        );
    }
    let mut line = String::new();
    if stdin().read_line(&mut line).unwrap_or(0) == 0 {
        eprintln!("no password was given on standard input");
        return 2;
    }
    // Only the line ending is removed: spaces can be part of a password.
    let password = line.strip_suffix('\n').unwrap_or(&line);
    let password = password.strip_suffix('\r').unwrap_or(password);

    match features::user::create_account(db, &email, password).await {
        Ok(Ok(user)) => {
            println!("created account {}", user.email);
            0
        }
        Ok(Err(error)) => {
            eprintln!("could not create the account: {}", error.message());
            1
        }
        Err(error) => {
            eprintln!("could not create the account: {error}");
            1
        }
    }
}

/// `todo-app delete-user <email> [--yes]`: deletes an account and everything it owns (todos,
/// lists, sessions), for the operator of a site where people cannot do it themselves. Without
/// `--yes` nothing is deleted: it only says what would be. Returns the process exit code.
async fn delete_user(db: &db::Db, args: impl Iterator<Item = String>) -> i32 {
    use features::{account, user::User, user::normalize_email};

    let usage = "usage: todo-app delete-user <email> [--yes]";
    let (mut email, mut confirmed) = (None, false);
    for arg in args.skip(1) {
        match arg.as_str() {
            "--yes" => confirmed = true,
            other if email.is_none() && !other.starts_with('-') => email = Some(arg),
            _ => {
                eprintln!("{usage}");
                return 2;
            }
        }
    }
    let Some(email) = email else {
        eprintln!("{usage}");
        return 2;
    };

    let found = match normalize_email(&email) {
        Some(email) => User::find_by_email(db, &email).await,
        None => Ok(None),
    };
    let user = match found {
        Ok(Some(user)) => user,
        Ok(None) => {
            eprintln!("there is no account with the email address {email:?}");
            return 1;
        }
        Err(error) => {
            eprintln!("could not look the account up: {error}");
            return 1;
        }
    };

    let owned = match account::owned_by(db, user.id).await {
        Ok(owned) => owned,
        Err(error) => {
            eprintln!("could not read the account's data: {error}");
            return 1;
        }
    };
    let summary = format!(
        "{} ({} todos, {} lists, {} sessions)",
        user.email, owned.todos, owned.lists, owned.sessions
    );
    if !confirmed {
        println!("nothing was deleted. This would permanently delete {summary}.");
        println!("Run the command again with --yes to delete it.");
        return 2;
    }

    match account::purge_user(db, user.id).await {
        Ok(_) => {
            println!("deleted account {summary}");
            0
        }
        Err(error) => {
            eprintln!("could not delete the account (nothing was changed): {error}");
            1
        }
    }
}

/// `todo-app reset-link <email>`: makes a one-time password reset link for an account and prints
/// it, for the operator to give to the person (the site cannot send email). The link works once
/// for an hour, and a newer link for the same account cancels it. It carries a secret, so
/// treat it like a password. Returns the process exit code.
async fn reset_link(
    db: &db::Db,
    public_url: Option<&str>,
    mut args: impl Iterator<Item = String>,
) -> i32 {
    use features::{
        reset,
        user::{User, normalize_email},
    };

    args.next(); // the subcommand's own name
    let (Some(email), None) = (args.next(), args.next()) else {
        eprintln!("usage: todo-app reset-link <email>");
        return 2;
    };

    let found = match normalize_email(&email) {
        Some(email) => User::find_by_email(db, &email).await,
        None => Ok(None),
    };
    let user = match found {
        Ok(Some(user)) => user,
        Ok(None) => {
            eprintln!("there is no account with the email address {email:?}");
            return 1;
        }
        Err(error) => {
            eprintln!("could not look the account up: {error}");
            return 1;
        }
    };

    let token = match reset::issue(db, user.id, reset::now()).await {
        Ok(token) => token,
        Err(error) => {
            eprintln!("could not make a reset link: {error}");
            return 1;
        }
    };
    let path = format!("/reset?token={token}");
    println!(
        "Password reset link for {} (works once, for 60 minutes; a newer link cancels it):",
        user.email
    );
    match public_url {
        Some(base) => println!("{base}{path}"),
        None => {
            println!("{path}");
            eprintln!(
                "note: PUBLIC_URL is not set, so put your site's address in front of that path."
            );
        }
    }
    0
}

/// The email sender, if `SMTP_URL` is set.
fn build_mail_sender(
    config: &config::Config,
) -> Result<Option<features::forgot::EmailSender>, String> {
    let (Some(email), Some(public_url)) = (&config.email, &config.public_url) else {
        return Ok(None);
    };
    features::forgot::EmailSender::from_config(email, public_url).map(Some)
}

/// `todo-app send-test-email <address>`: sends one message through the configured mail server,
/// so SMTP settings can be checked without waiting for someone to forget a password. It
/// contains no account data and no link. Returns the process exit code.
async fn send_test_email(
    mail: Option<features::forgot::EmailSender>,
    mut args: impl Iterator<Item = String>,
) -> i32 {
    args.next(); // the subcommand's own name
    let (Some(to), None) = (args.next(), args.next()) else {
        eprintln!("usage: todo-app send-test-email <address>");
        return 2;
    };
    let Some(mail) = mail else {
        eprintln!("email is not configured: set SMTP_URL, MAIL_FROM and PUBLIC_URL");
        return 2;
    };
    match mail.send_test(&to).await {
        Ok(()) => {
            println!("the mail server accepted a test message for {to}");
            0
        }
        Err(error) => {
            eprintln!("could not send the test message: {error}");
            1
        }
    }
}

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1).peekable();

    // `todo-app healthcheck` asks the running server if it is healthy. It needs no
    // configuration or database, so it runs first.
    if args.peek().is_some_and(|arg| arg == "healthcheck") {
        exit(if healthcheck::run() { 0 } else { 1 });
    }

    let config = config::Config::from_env().unwrap_or_else(|error| {
        eprintln!("configuration error: {error}");
        exit(2);
    });

    let db = db::connect(&config.database_url)
        .await
        .unwrap_or_else(|error| {
            eprintln!("cannot open the database: {error}");
            exit(1);
        });

    if config.migrate_on_start {
        match db::migrate(&db).await {
            Ok(0) => {}
            Ok(applied) => println!("applied {applied} database migration(s)"),
            Err(error) => {
                eprintln!("database migration failed: {error:#}");
                exit(1);
            }
        }
    }

    // Runs after the migrations, so it also works against a brand-new database.
    if args.peek().is_some_and(|arg| arg == "create-user") {
        exit(create_user(&db, args).await);
    }

    if args.peek().is_some_and(|arg| arg == "delete-user") {
        exit(delete_user(&db, args).await);
    }
    if args.peek().is_some_and(|arg| arg == "reset-link") {
        exit(reset_link(&db, config.public_url.as_deref(), args).await);
    }

    // Email is optional; when it is configured, a mistake is better found now than when
    // someone first asks for a link.
    let mail = build_mail_sender(&config).unwrap_or_else(|error| {
        eprintln!("configuration error: {error}");
        exit(2);
    });

    if args.peek().is_some_and(|arg| arg == "send-test-email") {
        exit(send_test_email(mail, args).await);
    }

    // Deletes expired session rows now, then once an hour.
    tokio::spawn(features::user::sweep_expired_sessions(db.clone()));

    topcoat::start(app::router(&config, mail).with(db).build())
        .await
        .unwrap();
}
