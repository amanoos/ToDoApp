//! The database: SQL Server, reached through Microsoft's `mssql-tds` driver (the `mssql-rs`
//! repository) by way of `mssql-tiberius-bridge`, which adds parameter binding and pooling.
//!
//! The rest of the app writes plain T-SQL with positional `@P1, @P2, ...` parameters and goes
//! through [`Db`]. Values are always sent as parameters, never pasted into the SQL text.
//! Primary keys are `BIGINT IDENTITY` columns; the app's ids are `u64`, and [`RowExt::id`] and
//! [`id_param`] convert at the boundary.
//!
//! The tables come from the numbered files in `migrations/`, compiled into the executable and
//! applied at startup by [`migrate`].

use std::fmt;

use mssql_tiberius_bridge::{
    AuthMethod, Client, Config, EncryptionLevel, FromSql, Pool, PooledConnection, Row, TdsManager,
    ToSql,
};
use topcoat::context::{Cx, context};

/// The most connections the pool opens at once.
const MAX_CONNECTIONS: usize = 10;

/// Every migration, in order: a name that is recorded once applied, and the T-SQL batch.
/// Add a new file to `migrations/` and a line here; never edit one that has been released.
const MIGRATIONS: &[(&str, &str)] = &[(
    "0001_initial",
    include_str!("../migrations/0001_initial.sql"),
)];

// --- Errors ------------------------------------------------------------------------

/// Anything that went wrong talking to the database.
#[derive(Debug)]
pub enum DbError {
    /// The database URL is not usable. The message never contains the URL, which holds a
    /// password.
    Config(String),
    /// The driver reported an error: a connection problem, bad SQL, a constraint violation.
    Driver(mssql_tiberius_bridge::Error),
    /// No connection could be taken from the pool.
    Pool(String),
    /// A column was missing, null where a value was required, or of an unexpected type.
    Column(String),
}

impl fmt::Display for DbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(message) => write!(f, "{message}"),
            Self::Driver(error) => write!(f, "{error}"),
            Self::Pool(message) => write!(f, "could not get a database connection: {message}"),
            Self::Column(column) => write!(f, "unexpected value in database column {column:?}"),
        }
    }
}

impl std::error::Error for DbError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Driver(error) => Some(error),
            _ => None,
        }
    }
}

impl From<mssql_tiberius_bridge::Error> for DbError {
    fn from(error: mssql_tiberius_bridge::Error) -> Self {
        Self::Driver(error)
    }
}

// --- The connection URL ------------------------------------------------------------

/// Where the database is and how to sign in, read from a `DATABASE_URL` such as
/// `sqlserver://sa:password@localhost:1433/todoapp?trust_server_certificate=true`.
///
/// The options are `trust_server_certificate` (`true` accepts a self-signed certificate, as
/// in the SQL Server container; default `false`) and `encrypt` (`true`, the default, `false`
/// or `strict`). The user name, password and database name may be percent-encoded.
#[derive(Clone, PartialEq, Eq)]
pub struct Target {
    host: String,
    port: u16,
    database: String,
    user: String,
    password: String,
    trust_certificate: bool,
    encryption: EncryptionLevel,
}

// Not derived: the password must never end up in a log.
impl fmt::Debug for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Target")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("database", &self.database)
            .finish_non_exhaustive()
    }
}

/// Decodes `%XX` escapes. Returns `None` for a broken escape or bytes that are not UTF-8.
fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = text.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

impl Target {
    /// Parses a database URL. The error says what is wrong without repeating the URL.
    pub fn parse(url: &str) -> Result<Self, String> {
        let bad = |what: &str| {
            format!("it must look like sqlserver://user:password@host:1433/database ({what})")
        };

        let rest = url
            .strip_prefix("sqlserver://")
            .ok_or_else(|| bad("it must start with \"sqlserver://\""))?;
        let (rest, query) = rest.split_once('?').unwrap_or((rest, ""));
        let (credentials, rest) = rest
            .rsplit_once('@')
            .ok_or_else(|| bad("the user name and password are missing"))?;
        let (user, password) = credentials
            .split_once(':')
            .ok_or_else(|| bad("the password is missing"))?;
        let (authority, database) = rest
            .split_once('/')
            .ok_or_else(|| bad("the database name is missing"))?;

        let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
            // An IPv6 address: [::1] or [::1]:1433
            let (host, after) = bracketed
                .split_once(']')
                .ok_or_else(|| bad("unclosed bracket in the host"))?;
            (host, after.strip_prefix(':'))
        } else {
            match authority.split_once(':') {
                Some((host, port)) => (host, Some(port)),
                None => (authority, None),
            }
        };
        let port = match port {
            None | Some("") => 1433,
            Some(port) => port
                .parse::<u16>()
                .map_err(|_| bad("the port is not a number"))?,
        };

        let database =
            percent_decode(database).ok_or_else(|| bad("the database name is not valid"))?;
        let user = percent_decode(user).ok_or_else(|| bad("the user name is not valid"))?;
        let password = percent_decode(password).ok_or_else(|| bad("the password is not valid"))?;
        if host.is_empty() || database.is_empty() || user.is_empty() || password.is_empty() {
            return Err(bad("the host, user, password and database cannot be empty"));
        }
        if database.len() > 100 || database.chars().any(char::is_control) {
            return Err(bad("the database name is not valid"));
        }

        let mut trust_certificate = false;
        let mut encryption = EncryptionLevel::On;
        for pair in query.split('&').filter(|pair| !pair.is_empty()) {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            match (key, value) {
                ("trust_server_certificate", "true") => trust_certificate = true,
                ("trust_server_certificate", "false") => trust_certificate = false,
                ("encrypt", "true") => encryption = EncryptionLevel::On,
                ("encrypt", "false") => encryption = EncryptionLevel::Off,
                ("encrypt", "strict") => encryption = EncryptionLevel::Strict,
                _ => return Err(bad("unknown option, or an option with a bad value")),
            }
        }

        Ok(Self {
            host: host.to_owned(),
            port,
            database,
            user,
            password,
            trust_certificate,
            encryption,
        })
    }

    /// The same server and login, another database.
    #[cfg(test)]
    pub fn with_database(&self, database: &str) -> Self {
        Self {
            database: database.to_owned(),
            ..self.clone()
        }
    }

    fn config(&self, database: &str) -> Config {
        let mut config = Config::new();
        config
            .host(&self.host)
            .port(self.port)
            .database(database)
            .authentication(AuthMethod::sql_server(&self.user, &self.password))
            .encryption(self.encryption)
            .client_name("todo-app");
        if self.trust_certificate {
            config.trust_cert();
        }
        config
    }
}

// --- The handle --------------------------------------------------------------------

/// A handle to the database: a connection pool. Cloning it is cheap and every clone shares
/// the same pool.
#[derive(Clone)]
pub struct Db {
    pool: Pool,
    /// In tests, drops the throwaway database when the last handle goes away.
    #[cfg(test)]
    _scratch: Option<std::sync::Arc<testing::ScratchDatabase>>,
}

impl Db {
    fn new(pool: Pool) -> Self {
        Self {
            pool,
            #[cfg(test)]
            _scratch: None,
        }
    }

    async fn connection(&self) -> Result<PooledConnection, DbError> {
        self.pool
            .get()
            .await
            .map_err(|error| DbError::Pool(error.to_string()))
    }

    /// Runs a query and returns the rows of its first result set. Parameters are `@P1`,
    /// `@P2`, ... in order.
    pub async fn query(&self, sql: &str, params: &[&dyn ToSql]) -> Result<Vec<Row>, DbError> {
        let mut connection = self.connection().await?;
        Ok(connection.query(sql, params).await?.into_first_result())
    }

    /// Like [`Db::query`], for at most one row.
    pub async fn query_one(
        &self,
        sql: &str,
        params: &[&dyn ToSql],
    ) -> Result<Option<Row>, DbError> {
        Ok(self.query(sql, params).await?.into_iter().next())
    }

    /// Runs a statement and returns how many rows it changed.
    pub async fn execute(&self, sql: &str, params: &[&dyn ToSql]) -> Result<u64, DbError> {
        let mut connection = self.connection().await?;
        Ok(connection.execute(sql, params).await?.total())
    }

    /// Starts a transaction on its own connection. Nothing is kept unless [`Tx::commit`] is
    /// called.
    pub async fn begin(&self) -> Result<Tx, DbError> {
        let mut connection = self.connection().await?;
        // XACT_ABORT makes a failed statement roll the whole transaction back.
        connection
            .simple_query("SET XACT_ABORT ON; BEGIN TRANSACTION;")
            .await?;
        Ok(Tx {
            connection: Some(connection),
        })
    }
}

/// An open transaction. Dropping it without calling [`Tx::commit`] undoes everything: the
/// connection is closed rather than returned to the pool, so the server rolls the work back
/// and releases its locks at once.
pub struct Tx {
    connection: Option<PooledConnection>,
}

impl Tx {
    fn connection(&mut self) -> &mut PooledConnection {
        self.connection
            .as_mut()
            .expect("a transaction's connection is only taken by commit or drop")
    }

    /// Like [`Db::query`], inside the transaction.
    pub async fn query(&mut self, sql: &str, params: &[&dyn ToSql]) -> Result<Vec<Row>, DbError> {
        Ok(self
            .connection()
            .query(sql, params)
            .await?
            .into_first_result())
    }

    /// Like [`Db::query_one`], inside the transaction.
    pub async fn query_one(
        &mut self,
        sql: &str,
        params: &[&dyn ToSql],
    ) -> Result<Option<Row>, DbError> {
        Ok(self.query(sql, params).await?.into_iter().next())
    }

    /// Like [`Db::execute`], inside the transaction.
    pub async fn execute(&mut self, sql: &str, params: &[&dyn ToSql]) -> Result<u64, DbError> {
        Ok(self.connection().execute(sql, params).await?.total())
    }

    /// Makes the transaction's changes permanent.
    pub async fn commit(mut self) -> Result<(), DbError> {
        self.connection()
            .simple_query("COMMIT TRANSACTION;")
            .await?;
        // Committed: the connection is clean and goes back to the pool when `self` drops.
        // (Drop closes it instead if the commit above failed and returned early.)
        if let Some(connection) = self.connection.take() {
            drop(connection);
        }
        Ok(())
    }
}

impl Drop for Tx {
    fn drop(&mut self) {
        // Reaching here with a connection still inside means no commit happened. Detach it
        // from the pool and let it close, which rolls the transaction back.
        if let Some(connection) = self.connection.take() {
            drop(PooledConnection::take(connection));
        }
    }
}

// --- Reading rows ------------------------------------------------------------------

/// Convenience readers for [`Row`] columns that report a [`DbError`] instead of `None`.
pub trait RowExt {
    /// A column that must hold a value.
    fn req<'a, T: FromSql<'a>>(&'a self, column: &str) -> Result<T, DbError>;
    /// A column that may be `NULL`.
    fn opt<'a, T: FromSql<'a>>(&'a self, column: &str) -> Result<Option<T>, DbError>;
    /// A `BIGINT` key as the app's `u64` id.
    fn id(&self, column: &str) -> Result<u64, DbError> {
        let value: i64 = self.req(column)?;
        u64::try_from(value).map_err(|_| DbError::Column(column.to_owned()))
    }
    /// A `BIGINT` key that may be `NULL`.
    fn opt_id(&self, column: &str) -> Result<Option<u64>, DbError> {
        self.opt::<i64>(column)?
            .map(|value| u64::try_from(value).map_err(|_| DbError::Column(column.to_owned())))
            .transpose()
    }
}

impl RowExt for Row {
    fn req<'a, T: FromSql<'a>>(&'a self, column: &str) -> Result<T, DbError> {
        self.try_get::<T, _>(column)?
            .ok_or_else(|| DbError::Column(column.to_owned()))
    }

    fn opt<'a, T: FromSql<'a>>(&'a self, column: &str) -> Result<Option<T>, DbError> {
        Ok(self.try_get::<Option<T>, _>(column)?.flatten())
    }
}

/// An id as a SQL parameter. An id too large for `BIGINT` cannot exist in the database, so it
/// becomes `-1`, which matches nothing.
pub fn id_param(id: u64) -> i64 {
    i64::try_from(id).unwrap_or(-1)
}

// --- Connecting and migrating ------------------------------------------------------

fn pool(target: &Target) -> Result<Pool, DbError> {
    TdsManager::create_pool(target.config(&target.database), MAX_CONNECTIONS)
        .map_err(|error| DbError::Config(format!("cannot set up the connection pool: {error}")))
}

/// Creates the database if it does not exist yet, by signing in to `master`. Needs a login
/// that may create databases.
async fn create_database(target: &Target) -> Result<(), DbError> {
    let mut client = Client::connect(&target.config("master")).await?;
    client
        .execute(
            "DECLARE @sql NVARCHAR(400) = N'CREATE DATABASE ' + QUOTENAME(@P1); \n             IF DB_ID(@P1) IS NULL EXEC(@sql);",
            &[&target.database.as_str()],
        )
        .await?;
    Ok(())
}

/// Opens the pool for the database at `url` (see [`Target`]) and checks that it can be
/// reached. If the database does not exist yet, it is created.
///
/// The tables come from migrations, which the app applies at startup (see [`migrate`]).
pub async fn connect(url: &str) -> Result<Db, DbError> {
    let target = Target::parse(url)
        .map_err(|problem| DbError::Config(format!("DATABASE_URL is not usable: {problem}")))?;
    connect_to(&target).await
}

async fn connect_to(target: &Target) -> Result<Db, DbError> {
    let db = Db::new(pool(target)?);
    if let Err(first) = db.connection().await {
        // Perhaps the database is missing. Try to create it, and report the original
        // problem if that does not help.
        if create_database(target).await.is_err() {
            return Err(first);
        }
        drop(db.connection().await?);
    }
    Ok(db)
}

/// Applies the migrations this database hasn't seen yet and returns how many that was.
/// Running it again changes nothing. All of it happens in one transaction under a lock, so
/// two copies of the app starting together cannot both apply the same migration.
pub async fn migrate(db: &Db) -> Result<usize, DbError> {
    let mut tx = db.begin().await?;
    tx.execute(
        "EXEC sp_getapplock @Resource = N'todo-app-migrations', @LockMode = N'Exclusive', \
         @LockOwner = N'Transaction', @LockTimeout = 60000",
        &[],
    )
    .await?;
    tx.execute(
        "IF OBJECT_ID(N'dbo.schema_migrations', N'U') IS NULL \
         CREATE TABLE dbo.schema_migrations ( \
             name NVARCHAR(200) NOT NULL CONSTRAINT pk_schema_migrations PRIMARY KEY, \
             applied_at BIGINT NOT NULL)",
        &[],
    )
    .await?;

    let mut applied = Vec::new();
    for row in tx
        .query("SELECT name FROM dbo.schema_migrations", &[])
        .await?
    {
        applied.push(row.req::<String>("name")?);
    }

    let now = crate::features::reset::now();
    let mut count = 0;
    for (name, sql) in MIGRATIONS {
        if applied.iter().any(|done| done == name) {
            continue;
        }
        tx.execute(sql, &[]).await?;
        tx.execute(
            "INSERT INTO dbo.schema_migrations (name, applied_at) VALUES (@P1, @P2)",
            &[name, &now],
        )
        .await?;
        count += 1;
    }
    tx.commit().await?;
    Ok(count)
}

/// Clones the database handle stored in the context. The clone uses the same connection
/// pool, so it is cheap to create.
pub fn db(cx: &Cx) -> Db {
    context::<Db>(cx).clone()
}

// --- Test support ------------------------------------------------------------------

/// A scratch database per test, on the SQL Server named by `TEST_DATABASE_URL` (default: the
/// local container from `docker-compose.dev.yml`). Each test gets its own, so tests can run
/// in parallel and each starts with empty tables.
#[cfg(test)]
pub mod testing {
    use std::sync::Arc;

    use argon2::password_hash::rand_core::{OsRng, RngCore};

    use super::*;

    /// A database created for one test and dropped with the last handle to it.
    pub struct ScratchDatabase {
        server: Target,
        name: String,
    }

    impl ScratchDatabase {
        /// Takes responsibility for dropping the database `name` on `server` when this value
        /// is dropped.
        pub fn new(server: Target, name: String) -> Self {
            Self { server, name }
        }
    }

    impl Drop for ScratchDatabase {
        fn drop(&mut self) {
            // Dropping needs an async connection, and this may run inside a runtime, so use a
            // thread of its own. Waiting for it keeps scratch databases from piling up.
            let (server, name) = (self.server.clone(), self.name.clone());
            let cleanup = std::thread::spawn(move || {
                let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                else {
                    return;
                };
                runtime.block_on(async move {
                    let Ok(mut client) = Client::connect(&server.config("master")).await else {
                        return;
                    };
                    let quoted = format!("[{name}]");
                    let _ = client
                        .simple_query(format!(
                            "IF DB_ID(N'{name}') IS NOT NULL BEGIN \
                             ALTER DATABASE {quoted} SET SINGLE_USER WITH ROLLBACK IMMEDIATE; \
                             DROP DATABASE {quoted}; END"
                        ))
                        .await;
                });
            });
            let _ = cleanup.join();
        }
    }

    /// The server the tests run against.
    pub fn server() -> Target {
        let url = std::env::var("TEST_DATABASE_URL")
            .unwrap_or_else(|_| crate::config::DEFAULT_DATABASE_URL.to_owned());
        Target::parse(&url).expect("TEST_DATABASE_URL is a valid sqlserver:// URL")
    }

    /// A name for a scratch database that no other test uses.
    pub fn scratch_name(purpose: &str) -> String {
        let mut random = [0u8; 8];
        OsRng.fill_bytes(&mut random);
        let random: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        format!("todo_test_{purpose}_{random}")
    }

    /// A new, empty database with no tables, as a new deployment starts.
    pub async fn blank() -> Db {
        let server = server();
        let name = scratch_name("db");
        let scratch = server.with_database(&name);
        create_database(&scratch).await.unwrap_or_else(|error| {
            panic!(
                "cannot create a test database: {error}\n\
                 Tests need SQL Server. Start it with:\n  \
                 docker compose -f docker-compose.dev.yml up -d --wait\n\
                 or set TEST_DATABASE_URL to another server."
            )
        });
        let mut db = Db::new(pool(&scratch).expect("the test pool is valid"));
        db._scratch = Some(Arc::new(ScratchDatabase::new(server, name)));
        db
    }

    /// A new database with every table created.
    pub async fn migrated() -> Db {
        let db = blank().await;
        migrate(&db).await.expect("migrations apply");
        db
    }
}

/// A context backed by a fresh scratch database with every table created, plus a new rate
/// limiter, for tests of code that takes a `&Cx`.
#[cfg(test)]
pub async fn test_cx() -> Cx {
    Cx::new()
        .with(testing::migrated().await)
        .with(crate::features::throttle::Throttle::new())
        .with(crate::config::Registration::Open)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::{
        list::List,
        todo::{Priority, Todo, TodoInput},
        user::SessionRecord,
    };

    #[test]
    fn a_database_url_is_taken_apart() {
        let target = Target::parse(
            "sqlserver://sa:p%40ss%3Aword@db.example.com:1444/todo%20app?trust_server_certificate=true&encrypt=strict",
        )
        .unwrap();
        assert_eq!(target.host, "db.example.com");
        assert_eq!(target.port, 1444);
        assert_eq!(target.database, "todo app");
        assert_eq!(target.user, "sa");
        assert_eq!(target.password, "p@ss:word");
        assert!(target.trust_certificate);
        assert_eq!(target.encryption, EncryptionLevel::Strict);
    }

    #[test]
    fn a_database_url_has_sensible_defaults() {
        let target = Target::parse("sqlserver://sa:secret@localhost/todo").unwrap();
        assert_eq!(target.port, 1433);
        assert!(!target.trust_certificate);
        assert_eq!(target.encryption, EncryptionLevel::On);

        let target = Target::parse("sqlserver://sa:secret@[::1]:1500/todo").unwrap();
        assert_eq!((target.host.as_str(), target.port), ("::1", 1500));
    }

    #[test]
    fn bad_database_urls_are_refused_without_echoing_them() {
        for bad in [
            "",
            "sqlite:data.db",
            "postgres://u:p@h/d",
            "sqlserver://localhost/todo",
            "sqlserver://sa@localhost/todo",
            "sqlserver://sa:hunter2@localhost",
            "sqlserver://sa:hunter2@localhost/",
            "sqlserver://sa:hunter2@:1433/todo",
            "sqlserver://sa:hunter2@localhost:notaport/todo",
            "sqlserver://sa:hunter2@localhost/todo?encrypt=maybe",
            "sqlserver://sa:hunter2@localhost/todo?surprise=1",
            "sqlserver://sa:%zz@localhost/todo",
        ] {
            let message = Target::parse(bad).expect_err(bad);
            assert!(!message.contains("hunter2"), "{message}");
        }
    }

    #[test]
    fn the_password_is_not_in_the_debug_output() {
        let target = Target::parse("sqlserver://sa:hunter2@localhost/todo").unwrap();
        assert!(!format!("{target:?}").contains("hunter2"));
    }

    /// If the schema and the code drift apart, this fails: it builds the schema only from the
    /// embedded migrations (as production does) and then uses every table and every column
    /// through the real data layer.
    #[tokio::test]
    async fn the_embedded_migrations_build_a_schema_the_code_can_use() {
        let db = testing::blank().await;
        let applied = migrate(&db).await.expect("migrations apply");
        assert_eq!(applied, MIGRATIONS.len());

        let cx = Cx::new().with(db.clone());

        let user = crate::features::user::create_account(
            &db,
            "someone@example.com",
            "a long enough password",
        )
        .await
        .expect("users table")
        .expect("account is created");
        SessionRecord::create(&db, &[9u8; 32], user.id, 123)
            .await
            .expect("session_records table");

        // The password_resets table: a link can be made and then found again.
        let link = crate::features::reset::issue(&db, user.id, 0)
            .await
            .expect("password_resets table");
        assert!(
            crate::features::reset::find_valid(&db, &link, 1)
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
        let db = testing::blank().await;
        assert_eq!(migrate(&db).await.unwrap(), MIGRATIONS.len());
        assert_eq!(migrate(&db).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn a_dropped_transaction_changes_nothing_and_a_committed_one_does() {
        let db = testing::migrated().await;
        let insert = "INSERT INTO dbo.lists (user_id, name) VALUES (1, N'x')";
        let count = || async {
            db.query("SELECT COUNT(*) AS n FROM dbo.lists", &[])
                .await
                .unwrap()[0]
                .req::<i32>("n")
                .unwrap()
        };

        let mut tx = db.begin().await.unwrap();
        tx.execute(insert, &[]).await.unwrap();
        drop(tx);
        assert_eq!(count().await, 0);

        let mut tx = db.begin().await.unwrap();
        tx.execute(insert, &[]).await.unwrap();
        tx.commit().await.unwrap();
        assert_eq!(count().await, 1);
    }

    #[tokio::test]
    async fn values_are_sent_as_parameters_not_pasted_into_the_sql() {
        let db = testing::migrated().await;
        let hostile = "x'); DROP TABLE dbo.lists; --";
        db.execute(
            "INSERT INTO dbo.lists (user_id, name) VALUES (@P1, @P2)",
            &[&1i64, &hostile],
        )
        .await
        .unwrap();
        let row = db
            .query_one("SELECT name FROM dbo.lists WHERE user_id = @P1", &[&1i64])
            .await
            .unwrap()
            .expect("the list is still there");
        assert_eq!(row.req::<String>("name").unwrap(), hostile);
    }

    #[tokio::test]
    async fn a_missing_database_is_created_on_connect() {
        let server = testing::server();
        let name = testing::scratch_name("created");
        let _cleanup = testing::ScratchDatabase::new(server.clone(), name.clone());

        let db = connect_to(&server.with_database(&name))
            .await
            .expect("the database is created");
        assert_eq!(migrate(&db).await.unwrap(), MIGRATIONS.len());
    }
}
