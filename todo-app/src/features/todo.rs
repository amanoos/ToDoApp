//! The todo list, stored in the database. Every todo belongs to one user, and every
//! function here takes the acting user's id, so one user can never touch another's todos.

use std::time::{SystemTime, UNIX_EPOCH};

use topcoat::{Result, context::Cx};

use super::list::List;
use crate::db::db;

/// The longest title a todo may have, in characters.
pub const MAX_TITLE_LEN: usize = 200;
/// The longest notes a todo may have, in characters.
pub const MAX_NOTES_LEN: usize = 2000;

/// A todo item.
#[derive(Debug, toasty::Model)]
pub struct Todo {
    #[key]
    #[auto]
    pub id: u64,

    /// The owner.
    #[index]
    pub user_id: u64,

    pub title: String,

    pub done: bool,

    /// Free text, or `None`.
    pub notes: Option<String>,

    /// `low`, `medium` or `high`; see [`Priority`].
    pub priority: String,

    /// The due date as `YYYY-MM-DD`, or `None`; see [`Date`].
    pub due_on: Option<String>,

    /// The list this todo is in, or `None` for the Inbox. Always one of the owner's lists.
    pub list_id: Option<u64>,
}

// --- Priority ---------------------------------------------------------------

/// How important a todo is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Priority {
    Low,
    #[default]
    Medium,
    High,
}

impl Priority {
    /// The stored and submitted form.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    /// The text shown to people.
    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
        }
    }

    /// Sort position: 0 for the most important.
    pub fn rank(self) -> u8 {
        match self {
            Self::High => 0,
            Self::Medium => 1,
            Self::Low => 2,
        }
    }

    /// Parses the stored form. Anything else is `None`.
    pub fn parse(value: &str) -> Option<Self> {
        [Self::Low, Self::Medium, Self::High]
            .into_iter()
            .find(|priority| priority.as_str() == value)
    }
}

// --- Dates ------------------------------------------------------------------

/// A calendar date with no time or time zone. Ordering is chronological.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Date {
    year: i32,
    month: u32,
    day: u32,
}

/// Whether a due date needs attention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DueStatus {
    Overdue,
    Today,
    Later,
}

impl Date {
    /// Parses `YYYY-MM-DD` strictly: real calendar dates only, years 1900 to 2200.
    pub fn parse(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
            return None;
        }
        let digits = |range: std::ops::Range<usize>| -> Option<u32> {
            let part = &text[range];
            part.bytes()
                .all(|b| b.is_ascii_digit())
                .then(|| part.parse().ok())
                .flatten()
        };
        let year = i32::try_from(digits(0..4)?).ok()?;
        let (month, day) = (digits(5..7)?, digits(8..10)?);
        let valid = (1900..=2200).contains(&year)
            && (1..=12).contains(&month)
            && (1..=days_in_month(year, month)).contains(&day);
        valid.then_some(Self { year, month, day })
    }

    /// Formats as `YYYY-MM-DD`.
    pub fn to_iso(self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    /// Today's date in UTC. A person far from UTC may see "today" change at an odd hour.
    pub fn today() -> Self {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        Self::from_unix_days(i64::try_from(seconds / 86_400).unwrap_or(0))
    }

    /// The date `days` days after 1970-01-01 (Howard Hinnant's `civil_from_days`).
    fn from_unix_days(days: i64) -> Self {
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let day_of_era = z.rem_euclid(146_097);
        let year_of_era =
            (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let shifted_month = (5 * day_of_year + 2) / 153;
        let day = u32::try_from(day_of_year - (153 * shifted_month + 2) / 5 + 1).unwrap_or(1);
        let month = u32::try_from(if shifted_month < 10 {
            shifted_month + 3
        } else {
            shifted_month - 9
        })
        .unwrap_or(1);
        let year = year_of_era + era * 400 + i64::from(month <= 2);
        Self {
            year: i32::try_from(year).unwrap_or(1970),
            month,
            day,
        }
    }

    /// How this date compares with `today`.
    pub fn status(self, today: Self) -> DueStatus {
        match self.cmp(&today) {
            std::cmp::Ordering::Less => DueStatus::Overdue,
            std::cmp::Ordering::Equal => DueStatus::Today,
            std::cmp::Ordering::Greater => DueStatus::Later,
        }
    }
}

fn days_in_month(year: i32, month: u32) -> u32 {
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

// --- Validated input --------------------------------------------------------

/// Which submitted field was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldError {
    Title,
    Notes,
    Priority,
    Due,
    List,
}

impl FieldError {
    /// The message shown on the form.
    pub fn message(self) -> &'static str {
        match self {
            Self::Title => "Enter a title of up to 200 characters.",
            Self::Notes => "Notes can be up to 2000 characters.",
            Self::Priority => "Choose low, medium or high.",
            Self::Due => "Enter a real date as YYYY-MM-DD, or leave it empty.",
            Self::List => "Choose one of your lists, or Inbox.",
        }
    }

    /// A short code carried in the redirect back to the form.
    pub fn code(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Notes => "notes",
            Self::Priority => "priority",
            Self::Due => "due",
            Self::List => "list",
        }
    }

    /// The inverse of [`FieldError::code`].
    pub fn from_code(code: &str) -> Option<Self> {
        [
            Self::Title,
            Self::Notes,
            Self::Priority,
            Self::Due,
            Self::List,
        ]
        .into_iter()
        .find(|error| error.code() == code)
    }
}

/// The editable fields of a todo, already checked. Build one with [`TodoInput::parse`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoInput {
    pub title: String,
    pub notes: Option<String>,
    pub priority: Priority,
    pub due: Option<Date>,
    /// The chosen list, or `None` for the Inbox. [`TodoInput::parse_with_list`] only
    /// checks that this is well-formed; [`Todo::add`] and [`Todo::save`] check that the
    /// user owns the list.
    pub list_id: Option<u64>,
}

/// Parses a submitted list choice: empty or `inbox` is the Inbox, digits are a list id.
fn parse_list_choice(value: &str) -> std::result::Result<Option<u64>, FieldError> {
    match value.trim() {
        "" | "inbox" => Ok(None),
        digits if digits.len() <= 20 && digits.bytes().all(|b| b.is_ascii_digit()) => {
            digits.parse().map(Some).map_err(|_| FieldError::List)
        }
        _ => Err(FieldError::List),
    }
}

impl TodoInput {
    /// Checks raw form text. Blank notes and a blank due date mean "none"; a blank
    /// priority means medium.
    pub fn parse(
        title: &str,
        notes: &str,
        priority: &str,
        due: &str,
    ) -> std::result::Result<Self, FieldError> {
        let title = title.trim();
        if title.is_empty() || title.chars().count() > MAX_TITLE_LEN {
            return Err(FieldError::Title);
        }

        let notes = notes.trim();
        if notes.chars().count() > MAX_NOTES_LEN {
            return Err(FieldError::Notes);
        }

        let priority = match priority.trim() {
            "" => Priority::default(),
            other => Priority::parse(other).ok_or(FieldError::Priority)?,
        };

        let due = match due.trim() {
            "" => None,
            other => Some(Date::parse(other).ok_or(FieldError::Due)?),
        };

        Ok(Self {
            title: title.to_owned(),
            notes: (!notes.is_empty()).then(|| notes.to_owned()),
            priority,
            due,
            list_id: None,
        })
    }

    /// Like [`TodoInput::parse`], and also reads the list choice (`""` or `inbox` for the
    /// Inbox, otherwise a list id).
    pub fn parse_with_list(
        title: &str,
        notes: &str,
        priority: &str,
        due: &str,
        list: &str,
    ) -> std::result::Result<Self, FieldError> {
        let mut input = Self::parse(title, notes, priority, due)?;
        input.list_id = parse_list_choice(list)?;
        Ok(input)
    }
}

// --- Filtering and search ---------------------------------------------------

/// The longest search text used, in characters. Longer text is cut off.
pub const MAX_QUERY_LEN: usize = 100;

/// Which todos to show, by completion.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    Active,
    Completed,
}

impl Filter {
    /// Every filter, in the order the tabs are shown.
    pub const ALL: [Filter; 3] = [Filter::All, Filter::Active, Filter::Completed];

    /// The form used in URLs.
    pub fn code(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Active => "active",
            Self::Completed => "completed",
        }
    }

    /// The text shown to people.
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Active => "Active",
            Self::Completed => "Completed",
        }
    }

    /// Parses the URL form. Anything else is `None`.
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|filter| filter.code() == value)
    }

    /// Whether a todo with this completion state is shown.
    pub fn shows(self, done: bool) -> bool {
        match self {
            Self::All => true,
            Self::Active => !done,
            Self::Completed => done,
        }
    }
}

/// How many todos each filter shows, ignoring any search text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub all: usize,
    pub active: usize,
    pub completed: usize,
}

impl Counts {
    pub fn of(todos: &[Todo]) -> Self {
        let completed = todos.iter().filter(|todo| todo.done).count();
        Self {
            all: todos.len(),
            active: todos.len() - completed,
            completed,
        }
    }

    pub fn get(self, filter: Filter) -> usize {
        match filter {
            Filter::All => self.all,
            Filter::Active => self.active,
            Filter::Completed => self.completed,
        }
    }
}

/// Trims search text and cuts it to [`MAX_QUERY_LEN`] characters.
pub fn clean_query(raw: &str) -> String {
    raw.trim()
        .chars()
        .take(MAX_QUERY_LEN)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Whether every word of the search appears in the title or the notes, ignoring case.
/// An empty search matches everything.
fn text_matches(title: &str, notes: Option<&str>, query: &str) -> bool {
    let haystack = format!("{} {}", title, notes.unwrap_or_default()).to_lowercase();
    query
        .to_lowercase()
        .split_whitespace()
        .all(|word| haystack.contains(word))
}

/// Keeps the todos that pass the filter and match the search, in their original order.
pub fn select(todos: Vec<Todo>, filter: Filter, query: &str) -> Vec<Todo> {
    select_in(todos, ListScope::All, filter, query)
}

/// Like [`select`], but only from the todos in `scope`.
pub fn select_in(todos: Vec<Todo>, scope: ListScope, filter: Filter, query: &str) -> Vec<Todo> {
    let query = clean_query(query);
    todos
        .into_iter()
        .filter(|todo| scope.contains(todo.list_id))
        .filter(|todo| filter.shows(todo.done))
        .filter(|todo| text_matches(&todo.title, todo.notes.as_deref(), &query))
        .collect()
}

/// Which list's todos to show.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ListScope {
    /// Every todo, whatever its list.
    #[default]
    All,
    /// Todos with no list.
    Inbox,
    /// Todos in one list.
    List(u64),
}

impl ListScope {
    /// The form used in URLs: `all`, `inbox`, or the list id.
    pub fn code(self) -> String {
        match self {
            Self::All => "all".to_owned(),
            Self::Inbox => "inbox".to_owned(),
            Self::List(id) => id.to_string(),
        }
    }

    /// Parses the URL form. Anything else is `None`.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "all" => Some(Self::All),
            "inbox" => Some(Self::Inbox),
            digits
                if digits.len() <= 20
                    && !digits.is_empty()
                    && digits.bytes().all(|b| b.is_ascii_digit()) =>
            {
                digits.parse().ok().map(Self::List)
            }
            _ => None,
        }
    }

    /// Whether a todo in `list_id` (`None` for the Inbox) is in this scope.
    pub fn contains(self, list_id: Option<u64>) -> bool {
        match self {
            Self::All => true,
            Self::Inbox => list_id.is_none(),
            Self::List(id) => list_id == Some(id),
        }
    }
}

/// How to order the todos.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Sort {
    /// Oldest first, which is the order they were added.
    #[default]
    Oldest,
    /// Newest first.
    Newest,
    /// Soonest due date first; todos with no due date come last.
    Due,
    /// High priority first, then medium, then low.
    Priority,
    /// Alphabetical by title, ignoring case.
    Title,
}

impl Sort {
    /// Every sort order, in the order the links are shown.
    pub const ALL: [Sort; 5] = [
        Sort::Oldest,
        Sort::Newest,
        Sort::Due,
        Sort::Priority,
        Sort::Title,
    ];

    /// The form used in URLs.
    pub fn code(self) -> &'static str {
        match self {
            Self::Oldest => "oldest",
            Self::Newest => "newest",
            Self::Due => "due",
            Self::Priority => "priority",
            Self::Title => "title",
        }
    }

    /// The text shown to people.
    pub fn label(self) -> &'static str {
        match self {
            Self::Oldest => "Oldest",
            Self::Newest => "Newest",
            Self::Due => "Due date",
            Self::Priority => "Priority",
            Self::Title => "Title",
        }
    }

    /// Parses the URL form. Anything else is `None`.
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|sort| sort.code() == value)
    }
}

/// Orders the todos. Ties always fall back to the order they were added, so the result is
/// stable and predictable.
pub fn sort_todos(todos: &mut [Todo], sort: Sort) {
    match sort {
        Sort::Oldest => todos.sort_by_key(|todo| todo.id),
        Sort::Newest => todos.sort_by_key(|todo| std::cmp::Reverse(todo.id)),
        Sort::Due => todos.sort_by_key(|todo| {
            let due = todo.due();
            (due.is_none(), due, todo.id)
        }),
        Sort::Priority => todos.sort_by_key(|todo| (todo.priority().rank(), todo.id)),
        Sort::Title => todos.sort_by_cached_key(|todo| (todo.title.to_lowercase(), todo.id)),
    }
}

// --- Queries ----------------------------------------------------------------

impl Todo {
    /// The priority, treating an unrecognised stored value as medium.
    pub fn priority(&self) -> Priority {
        Priority::parse(&self.priority).unwrap_or_default()
    }

    /// The due date, if one is set and readable.
    pub fn due(&self) -> Option<Date> {
        self.due_on.as_deref().and_then(Date::parse)
    }

    /// Returns the user's todos, oldest first.
    pub async fn list(cx: &Cx, user_id: u64) -> Result<Vec<Todo>> {
        let todos = Todo::filter_by_user_id(user_id)
            .order_by(Todo::fields().id().asc())
            .exec(&mut db(cx))
            .await?;
        Ok(todos)
    }

    /// Returns the user's todo with this id, or `None` if it doesn't exist or belongs to
    /// someone else.
    pub async fn find_owned(cx: &Cx, user_id: u64, id: u64) -> Result<Option<Todo>> {
        let todo = Todo::filter_by_id(id).first().exec(&mut db(cx)).await?;
        Ok(todo.filter(|todo| todo.user_id == user_id))
    }

    /// Whether the user may put a todo in `list_id`: the Inbox, or a list they own.
    async fn may_use_list(cx: &Cx, user_id: u64, list_id: Option<u64>) -> Result<bool> {
        match list_id {
            None => Ok(true),
            Some(id) => Ok(List::find_owned(cx, user_id, id).await?.is_some()),
        }
    }

    /// Adds a todo that is not done yet. Returns `false`, adding nothing, if the chosen
    /// list doesn't exist or belongs to someone else.
    pub async fn add(cx: &Cx, user_id: u64, input: &TodoInput) -> Result<bool> {
        if !Todo::may_use_list(cx, user_id, input.list_id).await? {
            return Ok(false);
        }
        let due_on = input.due.map(Date::to_iso);
        toasty::create!(Todo {
            user_id,
            title: input.title.as_str(),
            done: false,
            notes: input.notes.clone(),
            priority: input.priority.as_str(),
            due_on,
            list_id: input.list_id,
        })
        .exec(&mut db(cx))
        .await?;
        Ok(true)
    }

    /// Replaces the editable fields of the user's todo. Returns `false`, changing
    /// nothing, if the todo doesn't exist or belongs to someone else, or if the chosen
    /// list doesn't exist or belongs to someone else.
    pub async fn save(cx: &Cx, user_id: u64, id: u64, input: &TodoInput) -> Result<bool> {
        let Some(mut todo) = Todo::find_owned(cx, user_id, id).await? else {
            return Ok(false);
        };
        if !Todo::may_use_list(cx, user_id, input.list_id).await? {
            return Ok(false);
        }
        todo.update()
            .list_id(input.list_id)
            .title(input.title.as_str())
            .notes(input.notes.clone())
            .priority(input.priority.as_str())
            .due_on(input.due.map(Date::to_iso))
            .exec(&mut db(cx))
            .await?;
        Ok(true)
    }

    /// Marks the user's todo as done, or as not done if it already is. Does nothing if
    /// the todo doesn't exist or belongs to someone else.
    pub async fn toggle(cx: &Cx, user_id: u64, id: u64) -> Result<()> {
        let Some(mut todo) = Todo::find_owned(cx, user_id, id).await? else {
            return Ok(());
        };
        let done = !todo.done;
        todo.update().done(done).exec(&mut db(cx)).await?;
        Ok(())
    }

    /// Removes the user's todo. Does nothing if it doesn't exist or belongs to someone else.
    pub async fn remove(cx: &Cx, user_id: u64, id: u64) -> Result<()> {
        if Todo::find_owned(cx, user_id, id).await?.is_some() {
            Todo::filter_by_id(id).delete().exec(&mut db(cx)).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(text: &str) -> Date {
        Date::parse(text).unwrap_or_else(|| panic!("{text} should parse"))
    }

    #[test]
    fn valid_dates_round_trip() {
        for text in ["2026-11-09", "2024-02-29", "1900-01-01", "2200-12-31"] {
            assert_eq!(date(text).to_iso(), text);
        }
    }

    #[test]
    fn impossible_or_malformed_dates_are_rejected() {
        for bad in [
            "",
            "2026-11-9",
            "2026/11/09",
            "2026-13-01",
            "2026-00-10",
            "2026-04-31",
            "2023-02-29",
            "1899-12-31",
            "2201-01-01",
            "20261109",
            " 2026-11-09",
            "2026-11-09 ",
            "２０２６-11-09",
            "+026-11-09",
            "2026-1a-09",
        ] {
            assert_eq!(Date::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn leap_years() {
        assert!(Date::parse("2024-02-29").is_some());
        assert!(Date::parse("2000-02-29").is_some());
        assert!(Date::parse("1900-02-29").is_none());
        assert!(Date::parse("2100-02-29").is_none());
    }

    #[test]
    fn converts_unix_days_to_dates() {
        assert_eq!(Date::from_unix_days(0).to_iso(), "1970-01-01");
        assert_eq!(Date::from_unix_days(19_000).to_iso(), "2022-01-08");
        assert_eq!(Date::from_unix_days(19_782).to_iso(), "2024-02-29");
        assert_eq!(Date::from_unix_days(-1).to_iso(), "1969-12-31");
    }

    #[test]
    fn today_is_a_plausible_date() {
        let today = Date::today();
        assert!(today > date("2025-01-01"));
        assert!(today < date("2200-01-01"));
    }

    #[test]
    fn due_status_compares_with_today() {
        let today = date("2026-11-09");
        assert_eq!(date("2026-11-08").status(today), DueStatus::Overdue);
        assert_eq!(date("2026-11-09").status(today), DueStatus::Today);
        assert_eq!(date("2026-11-10").status(today), DueStatus::Later);
        assert_eq!(date("2025-12-31").status(today), DueStatus::Overdue);
        assert_eq!(date("2027-01-01").status(today), DueStatus::Later);
    }

    #[test]
    fn priority_round_trips() {
        for priority in [Priority::Low, Priority::Medium, Priority::High] {
            assert_eq!(Priority::parse(priority.as_str()), Some(priority));
        }
        assert_eq!(Priority::parse("HIGH"), None);
        assert_eq!(Priority::default(), Priority::Medium);
    }

    #[test]
    fn parses_a_full_input() {
        let input =
            TodoInput::parse("  Buy milk ", " from the shop ", "high", "2026-11-12").unwrap();
        assert_eq!(input.title, "Buy milk");
        assert_eq!(input.notes.as_deref(), Some("from the shop"));
        assert_eq!(input.priority, Priority::High);
        assert_eq!(input.due, Some(date("2026-11-12")));
    }

    #[test]
    fn blank_optional_fields_mean_none_and_medium() {
        let input = TodoInput::parse("Buy milk", "  ", "", " ").unwrap();
        assert_eq!(input.notes, None);
        assert_eq!(input.priority, Priority::Medium);
        assert_eq!(input.due, None);
    }

    #[test]
    fn each_bad_field_is_reported() {
        assert_eq!(
            TodoInput::parse("  ", "", "low", ""),
            Err(FieldError::Title)
        );
        let long_title = "t".repeat(MAX_TITLE_LEN + 1);
        assert_eq!(
            TodoInput::parse(&long_title, "", "low", ""),
            Err(FieldError::Title)
        );
        let long_notes = "n".repeat(MAX_NOTES_LEN + 1);
        assert_eq!(
            TodoInput::parse("x", &long_notes, "low", ""),
            Err(FieldError::Notes)
        );
        assert_eq!(
            TodoInput::parse("x", "", "urgent", ""),
            Err(FieldError::Priority)
        );
        assert_eq!(
            TodoInput::parse("x", "", "low", "2026-02-30"),
            Err(FieldError::Due)
        );
    }

    #[test]
    fn length_limits_count_characters_and_are_inclusive() {
        assert!(TodoInput::parse(&"é".repeat(MAX_TITLE_LEN), "", "low", "").is_ok());
        assert!(TodoInput::parse("x", &"é".repeat(MAX_NOTES_LEN), "low", "").is_ok());
    }

    // --- Ownership, against a real in-memory database ---

    const ALICE: u64 = 1;
    const BOB: u64 = 2;

    async fn add_for(cx: &Cx, user_id: u64, title: &str) -> u64 {
        let input = TodoInput::parse(title, "", "medium", "").expect("valid input");
        assert!(Todo::add(cx, user_id, &input).await.expect("added"));
        Todo::list(cx, user_id)
            .await
            .expect("listed")
            .last()
            .expect("the new todo is listed")
            .id
    }

    async fn reload(cx: &Cx, user_id: u64, id: u64) -> Option<Todo> {
        Todo::find_owned(cx, user_id, id).await.expect("queried")
    }

    #[tokio::test]
    async fn each_user_lists_only_their_own_todos() {
        let cx = crate::db::test_cx().await;
        add_for(&cx, ALICE, "alice 1").await;
        add_for(&cx, ALICE, "alice 2").await;
        add_for(&cx, BOB, "bob 1").await;

        let alice: Vec<_> = Todo::list(&cx, ALICE)
            .await
            .unwrap()
            .into_iter()
            .map(|t| t.title)
            .collect();
        let bob: Vec<_> = Todo::list(&cx, BOB)
            .await
            .unwrap()
            .into_iter()
            .map(|t| t.title)
            .collect();
        assert_eq!(alice, ["alice 1", "alice 2"]);
        assert_eq!(bob, ["bob 1"]);
        assert!(Todo::list(&cx, 99).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn another_user_cannot_find_a_todo() {
        let cx = crate::db::test_cx().await;
        let id = add_for(&cx, ALICE, "secret").await;
        assert!(reload(&cx, ALICE, id).await.is_some());
        assert!(reload(&cx, BOB, id).await.is_none());
    }

    #[tokio::test]
    async fn another_user_cannot_toggle_a_todo() {
        let cx = crate::db::test_cx().await;
        let id = add_for(&cx, ALICE, "secret").await;

        Todo::toggle(&cx, BOB, id).await.unwrap();
        assert!(
            !reload(&cx, ALICE, id).await.unwrap().done,
            "Bob's toggle changed nothing"
        );

        // Control: the owner's toggle works, so the test above is not passing by accident.
        Todo::toggle(&cx, ALICE, id).await.unwrap();
        assert!(reload(&cx, ALICE, id).await.unwrap().done);
        Todo::toggle(&cx, ALICE, id).await.unwrap();
        assert!(!reload(&cx, ALICE, id).await.unwrap().done);
    }

    #[tokio::test]
    async fn another_user_cannot_remove_a_todo() {
        let cx = crate::db::test_cx().await;
        let id = add_for(&cx, ALICE, "secret").await;

        Todo::remove(&cx, BOB, id).await.unwrap();
        assert!(
            reload(&cx, ALICE, id).await.is_some(),
            "Bob's remove deleted nothing"
        );

        Todo::remove(&cx, ALICE, id).await.unwrap();
        assert!(
            reload(&cx, ALICE, id).await.is_none(),
            "the owner can remove it"
        );
    }

    #[tokio::test]
    async fn another_user_cannot_edit_a_todo() {
        let cx = crate::db::test_cx().await;
        let id = add_for(&cx, ALICE, "original").await;
        let hostile = TodoInput::parse("HACKED", "pwned", "high", "2030-01-01").unwrap();

        assert!(
            !Todo::save(&cx, BOB, id, &hostile).await.unwrap(),
            "refused"
        );
        let todo = reload(&cx, ALICE, id).await.unwrap();
        assert_eq!(todo.title, "original");
        assert_eq!(todo.notes, None);
        assert_eq!(todo.priority(), Priority::Medium);
        assert_eq!(todo.due(), None);

        // Control: the owner can.
        let fine = TodoInput::parse("renamed", "n", "high", "2030-01-01").unwrap();
        assert!(Todo::save(&cx, ALICE, id, &fine).await.unwrap());
        let todo = reload(&cx, ALICE, id).await.unwrap();
        assert_eq!(
            (todo.title.as_str(), todo.priority()),
            ("renamed", Priority::High)
        );
    }

    #[tokio::test]
    async fn operations_on_missing_todos_are_harmless() {
        let cx = crate::db::test_cx().await;
        let id = add_for(&cx, ALICE, "keep").await;
        Todo::toggle(&cx, ALICE, id + 1000).await.unwrap();
        Todo::remove(&cx, ALICE, id + 1000).await.unwrap();
        let input = TodoInput::parse("x", "", "low", "").unwrap();
        assert!(!Todo::save(&cx, ALICE, id + 1000, &input).await.unwrap());
        assert_eq!(Todo::list(&cx, ALICE).await.unwrap().len(), 1);
    }

    // --- Lists and sorting ---

    #[test]
    fn list_choices_parse_and_bad_ones_are_rejected() {
        assert_eq!(parse_list_choice(""), Ok(None));
        assert_eq!(parse_list_choice("inbox"), Ok(None));
        assert_eq!(parse_list_choice(" 42 "), Ok(Some(42)));
        for bad in [
            "-1",
            "+5",
            "1.5",
            "abc",
            "42abc",
            "0x10",
            "INBOX",
            "all",
            "18446744073709551616",  // one past u64::MAX
            "999999999999999999999", // 21 digits
        ] {
            assert_eq!(parse_list_choice(bad), Err(FieldError::List), "{bad:?}");
        }
    }

    #[test]
    fn a_full_input_with_a_list_parses() {
        let input = TodoInput::parse_with_list("x", "", "low", "", "7").unwrap();
        assert_eq!(input.list_id, Some(7));
        let input = TodoInput::parse_with_list("x", "", "low", "", "inbox").unwrap();
        assert_eq!(input.list_id, None);
        assert_eq!(
            TodoInput::parse_with_list("x", "", "low", "", "nope"),
            Err(FieldError::List)
        );
        // The other fields are still checked first.
        assert_eq!(
            TodoInput::parse_with_list("", "", "low", "", "7"),
            Err(FieldError::Title)
        );
    }

    #[test]
    fn scope_codes_round_trip() {
        for scope in [ListScope::All, ListScope::Inbox, ListScope::List(7)] {
            assert_eq!(ListScope::parse(&scope.code()), Some(scope));
        }
        for bad in [
            "",
            "ALL",
            "-1",
            "12a",
            " 7",
            "7 ",
            "+7",
            "999999999999999999999",
        ] {
            assert_eq!(ListScope::parse(bad), None, "{bad:?}");
        }
        assert_eq!(ListScope::default(), ListScope::All);
    }

    #[test]
    fn scope_membership() {
        assert!(ListScope::All.contains(None) && ListScope::All.contains(Some(3)));
        assert!(ListScope::Inbox.contains(None) && !ListScope::Inbox.contains(Some(3)));
        assert!(ListScope::List(3).contains(Some(3)));
        assert!(!ListScope::List(3).contains(Some(4)) && !ListScope::List(3).contains(None));
    }

    fn in_list(id: u64, title: &str, done: bool, list_id: Option<u64>) -> Todo {
        Todo {
            list_id,
            ..sample(id, title, None, done)
        }
    }

    #[test]
    fn selection_combines_scope_filter_and_search() {
        let todos = || {
            vec![
                in_list(1, "milk", false, None),
                in_list(2, "milk run", true, Some(10)),
                in_list(3, "bread", false, Some(10)),
                in_list(4, "milk", false, Some(11)),
            ]
        };
        assert_eq!(
            ids(select_in(todos(), ListScope::All, Filter::All, "")),
            [1, 2, 3, 4]
        );
        assert_eq!(
            ids(select_in(todos(), ListScope::Inbox, Filter::All, "")),
            [1]
        );
        assert_eq!(
            ids(select_in(todos(), ListScope::List(10), Filter::All, "")),
            [2, 3]
        );
        assert_eq!(
            ids(select_in(todos(), ListScope::List(10), Filter::Active, "")),
            [3]
        );
        assert_eq!(
            ids(select_in(todos(), ListScope::List(10), Filter::All, "milk")),
            [2]
        );
        assert!(select_in(todos(), ListScope::List(99), Filter::All, "").is_empty());
        // `select` is the all-lists case.
        assert_eq!(ids(select(todos(), Filter::All, "milk")), [1, 2, 4]);
    }

    fn detailed(id: u64, title: &str, priority: &str, due: Option<&str>) -> Todo {
        Todo {
            priority: priority.to_owned(),
            due_on: due.map(str::to_owned),
            ..sample(id, title, None, false)
        }
    }

    fn order(sort: Sort) -> Vec<u64> {
        // Deliberately not in id order.
        let mut todos = vec![
            detailed(3, "banana", "low", Some("2026-12-01")),
            detailed(1, "Cherry", "high", None),
            detailed(4, "apple", "medium", Some("2026-11-15")),
            detailed(2, "Apple", "high", Some("2026-11-15")),
            detailed(5, "date", "bogus", Some("not-a-date")),
        ];
        sort_todos(&mut todos, sort);
        ids(todos)
    }

    #[test]
    fn sorts_by_age() {
        assert_eq!(order(Sort::Oldest), [1, 2, 3, 4, 5]);
        assert_eq!(order(Sort::Newest), [5, 4, 3, 2, 1]);
    }

    #[test]
    fn sorts_by_due_date_with_undated_last_and_ties_by_age() {
        // 2026-11-15 (ids 2, 4), then 2026-12-01 (id 3), then undated or unreadable (1, 5).
        assert_eq!(order(Sort::Due), [2, 4, 3, 1, 5]);
    }

    #[test]
    fn sorts_by_priority_with_unknown_counting_as_medium_and_ties_by_age() {
        // high: 1, 2   medium (incl. the unreadable one): 4, 5   low: 3
        assert_eq!(order(Sort::Priority), [1, 2, 4, 5, 3]);
    }

    #[test]
    fn sorts_by_title_ignoring_case_with_ties_by_age() {
        // "Apple" (2) and "apple" (4) tie ignoring case and keep id order.
        assert_eq!(order(Sort::Title), [2, 4, 3, 1, 5]);
    }

    #[test]
    fn sort_codes_round_trip_and_default_to_oldest() {
        for sort in Sort::ALL {
            assert_eq!(Sort::parse(sort.code()), Some(sort));
        }
        assert_eq!(Sort::parse("DUE"), None);
        assert_eq!(Sort::parse(""), None);
        assert_eq!(Sort::default(), Sort::Oldest);
    }

    #[tokio::test]
    async fn a_todo_cannot_be_added_to_someone_elses_list() {
        let cx = crate::db::test_cx().await;
        let bobs = List::add(&cx, BOB, "Bob's").await.unwrap().unwrap();
        let mine = List::add(&cx, ALICE, "Mine").await.unwrap().unwrap();
        let mut input = TodoInput::parse("x", "", "medium", "").unwrap();

        input.list_id = Some(bobs.id);
        assert!(
            !Todo::add(&cx, ALICE, &input).await.unwrap(),
            "someone else's list"
        );
        input.list_id = Some(bobs.id + 1000);
        assert!(
            !Todo::add(&cx, ALICE, &input).await.unwrap(),
            "a list that doesn't exist"
        );
        assert!(
            Todo::list(&cx, ALICE).await.unwrap().is_empty(),
            "nothing was added"
        );

        input.list_id = Some(mine.id);
        assert!(
            Todo::add(&cx, ALICE, &input).await.unwrap(),
            "control: her own list works"
        );
        assert_eq!(
            Todo::list(&cx, ALICE).await.unwrap()[0].list_id,
            Some(mine.id)
        );
    }

    #[tokio::test]
    async fn editing_cannot_move_a_todo_into_someone_elses_list() {
        let cx = crate::db::test_cx().await;
        let bobs = List::add(&cx, BOB, "Bob's").await.unwrap().unwrap();
        let mine = List::add(&cx, ALICE, "Mine").await.unwrap().unwrap();
        let id = add_for(&cx, ALICE, "todo").await;
        let mut input = TodoInput::parse("todo", "", "medium", "").unwrap();

        input.list_id = Some(bobs.id);
        assert!(!Todo::save(&cx, ALICE, id, &input).await.unwrap());
        assert_eq!(
            reload(&cx, ALICE, id).await.unwrap().list_id,
            None,
            "still in the Inbox"
        );

        input.list_id = Some(mine.id);
        assert!(Todo::save(&cx, ALICE, id, &input).await.unwrap(), "control");
        assert_eq!(reload(&cx, ALICE, id).await.unwrap().list_id, Some(mine.id));

        input.list_id = None;
        assert!(Todo::save(&cx, ALICE, id, &input).await.unwrap());
        assert_eq!(
            reload(&cx, ALICE, id).await.unwrap().list_id,
            None,
            "back to the Inbox"
        );
    }

    fn sample(id: u64, title: &str, notes: Option<&str>, done: bool) -> Todo {
        Todo {
            id,
            user_id: 1,
            title: title.to_owned(),
            done,
            notes: notes.map(str::to_owned),
            priority: "medium".to_owned(),
            due_on: None,
            list_id: None,
        }
    }

    fn samples() -> Vec<Todo> {
        vec![
            sample(1, "Buy milk", None, false),
            sample(2, "Write report", Some("Quarterly numbers for Dana"), true),
            sample(3, "Call Dana", Some("about the MILK order"), false),
            sample(4, "Pay rent", None, true),
        ]
    }

    fn ids(todos: Vec<Todo>) -> Vec<u64> {
        todos.into_iter().map(|todo| todo.id).collect()
    }

    #[test]
    fn filters_by_completion() {
        assert_eq!(ids(select(samples(), Filter::All, "")), [1, 2, 3, 4]);
        assert_eq!(ids(select(samples(), Filter::Active, "")), [1, 3]);
        assert_eq!(ids(select(samples(), Filter::Completed, "")), [2, 4]);
    }

    #[test]
    fn search_ignores_case_and_looks_in_notes() {
        assert_eq!(ids(select(samples(), Filter::All, "MILK")), [1, 3]);
        assert_eq!(ids(select(samples(), Filter::All, "quarterly")), [2]);
    }

    #[test]
    fn every_word_must_match_in_any_order() {
        assert_eq!(ids(select(samples(), Filter::All, "dana call")), [3]);
        assert_eq!(ids(select(samples(), Filter::All, "dana milk")), [3]);
        assert!(select(samples(), Filter::All, "milk rent").is_empty());
    }

    #[test]
    fn filter_and_search_combine() {
        assert_eq!(ids(select(samples(), Filter::Active, "dana")), [3]);
        assert_eq!(ids(select(samples(), Filter::Completed, "dana")), [2]);
        assert!(select(samples(), Filter::Completed, "milk").is_empty());
    }

    #[test]
    fn blank_search_matches_everything_and_no_match_is_empty() {
        assert_eq!(select(samples(), Filter::All, "   ").len(), 4);
        assert!(select(samples(), Filter::All, "zebra").is_empty());
    }

    #[test]
    fn search_text_is_plain_text_not_a_pattern() {
        for text in ["%", "_", ".*", "'; DROP TABLE todos; --", "[a-z]", "\\"] {
            assert!(select(samples(), Filter::All, text).is_empty(), "{text:?}");
        }
    }

    #[test]
    fn query_is_trimmed_and_capped() {
        assert_eq!(clean_query("  milk \n"), "milk");
        assert_eq!(
            clean_query(&"a".repeat(MAX_QUERY_LEN + 50)).chars().count(),
            MAX_QUERY_LEN
        );
        assert_eq!(
            clean_query(&"é".repeat(MAX_QUERY_LEN + 1)).chars().count(),
            MAX_QUERY_LEN
        );
    }

    #[test]
    fn counts_ignore_the_search() {
        let counts = Counts::of(&samples());
        assert_eq!(
            counts,
            Counts {
                all: 4,
                active: 2,
                completed: 2
            }
        );
        assert_eq!(counts.get(Filter::Active), 2);
        assert_eq!(Counts::of(&[]), Counts::default());
    }

    #[test]
    fn filter_codes_round_trip_and_default_to_all() {
        for filter in Filter::ALL {
            assert_eq!(Filter::parse(filter.code()), Some(filter));
        }
        assert_eq!(Filter::parse("ACTIVE"), None);
        assert_eq!(Filter::parse(""), None);
        assert_eq!(Filter::default(), Filter::All);
    }

    #[test]
    fn field_error_codes_round_trip() {
        for error in [
            FieldError::Title,
            FieldError::Notes,
            FieldError::Priority,
            FieldError::Due,
            FieldError::List,
        ] {
            assert_eq!(FieldError::from_code(error.code()), Some(error));
        }
        assert_eq!(FieldError::from_code("nope"), None);
    }
}
