// The Kitchen section: recipes, a cook mode for the counter, a week of meals
// and the shopping list it makes.
//
// Five tabs over one snapshot — Recipes, Recipe, Cook, Plan and Shopping. The
// parsing and the store are `crate::kitchen`; this file keeps the session
// (which tab, which recipe, how many servings, which step) and maps rows into
// what Dart draws. Timers count down in Dart, which is where the second hand
// is; the bridge only says which ones a step asks for.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, MutexGuard, OnceLock};

use anyhow::{Result, anyhow, bail};
use chrono::{Datelike, NaiveDate};
use sqlx::SqlitePool;

use crate::api::transfer::QrCode;
use crate::db::kitchen_pool;
use crate::kitchen::{self as k, AISLES};

// ------------------------------------------------------------------- state ---

pub struct KitchenState {
    /// recipes | recipe | cook | plan | shop | pantry.
    pub tab: String,
    pub notice: String,
    pub total: i64,
    /// all | fav | quick | veg | baking | canmake.
    pub filter: String,
    pub filters: Vec<FilterChip>,
    pub query: String,
    pub recipes: Vec<RecipeCard>,
    /// The recipe on the Recipe and Cook tabs.
    pub open: Option<RecipeView>,
    pub cooking: bool,
    /// The step on screen in cook mode, from 0.
    pub cook_step: i64,
    pub week: WeekView,
    pub shop: ShopView,
    /// Items not yet in the basket, for the tab's count.
    pub shop_left: i64,
    /// Finances accounts, for "Log in Finances".
    pub accounts: Vec<AccountPick>,
    /// Every recipe by name, for the plan's picker.
    pub picks: Vec<RecipePick>,
    /// What Kitchen thinks is in the kitchen, by aisle; on the Pantry tab.
    pub pantry: Vec<PantryGroup>,
    /// Pantry keys, for the tab's count.
    pub pantry_n: i64,
    /// Cook mode is listening for "next", "back", "repeat", "timer", "stop".
    pub listening: bool,
    /// The last word heard, and a count that moves each time one is, so the
    /// page can act on "repeat" or "timer" once.
    pub heard: String,
    pub heard_n: i64,
}

pub struct PantryGroup {
    pub aisle: String,
    /// The pantry's keys: "tomato", "olive oil".
    pub keys: Vec<String>,
}

pub struct FilterChip {
    pub id: String,
    pub label: String,
    pub n: i64,
}

pub struct RecipeCard {
    pub id: i64,
    pub title: String,
    /// A local file, or empty.
    pub image: String,
    /// "35 min", "1 h 30 m".
    pub time: String,
    pub category: String,
    pub favourite: bool,
    pub can_make: bool,
}

pub struct RecipeView {
    pub id: i64,
    pub title: String,
    pub image: String,
    pub time: String,
    pub minutes: i64,
    pub category: String,
    pub host: String,
    pub url: String,
    pub cooked: i64,
    pub favourite: bool,
    pub servings: i64,
    pub base_servings: i64,
    pub ingredients: Vec<IngRow>,
    pub steps: Vec<StepRow>,
    /// Ingredients not in stock.
    pub missing: i64,
}

pub struct IngRow {
    pub id: i64,
    /// "2 tbsp", scaled; empty when the line has no quantity.
    pub amount: String,
    pub name: String,
    pub note: String,
    pub key: String,
    pub in_stock: bool,
    /// Salt, pepper, water: always there, no box to tick.
    pub staple: bool,
}

pub struct StepRow {
    pub text: String,
    pub timers: Vec<TimerSpec>,
    /// "800 g chopped tomatoes", for cook mode.
    pub uses: Vec<String>,
}

pub struct TimerSpec {
    pub label: String,
    pub secs: i64,
}

#[derive(Default)]
pub struct WeekView {
    /// The Monday, ISO.
    pub start: String,
    /// "This week", "Next week", "Week of 21 September".
    pub title: String,
    /// "14–20 September".
    pub range: String,
    pub planned: i64,
    pub days: Vec<PlanDay>,
}

pub struct PlanDay {
    pub day: String,
    /// "Mon".
    pub dow: String,
    pub n: i64,
    pub today: bool,
    pub past: bool,
    pub lunch: PlanSlot,
    pub dinner: PlanSlot,
}

#[derive(Default)]
pub struct PlanSlot {
    /// 0 for none.
    pub recipe_id: i64,
    pub title: String,
    pub image: String,
    /// Free text: "Out", "Leftover tacos".
    pub label: String,
}

#[derive(Default)]
pub struct ShopView {
    /// Planned meals the list was made from; 0 when it was not made from a plan.
    pub from_meals: i64,
    pub in_basket: i64,
    pub total: i64,
    pub groups: Vec<ShopGroup>,
}

pub struct ShopGroup {
    pub aisle: String,
    pub items: Vec<ShopItem>,
}

pub struct ShopItem {
    pub id: i64,
    pub name: String,
    pub amount: String,
    pub done: bool,
}

pub struct AccountPick {
    pub id: i64,
    pub name: String,
    pub currency: String,
}

pub struct RecipePick {
    pub id: i64,
    pub title: String,
}

/// A recipe as the editor shows it: one ingredient per line, one step per line.
pub struct RecipeDraft {
    pub id: i64,
    pub title: String,
    pub minutes: i64,
    pub servings: i64,
    pub category: String,
    pub ingredients: String,
    pub steps: String,
    pub source_url: String,
}

pub struct PhoneLink {
    pub url: String,
    pub qr: Option<QrCode>,
}

// ---------------------------------------------------------------- commands ---

pub enum KitchenCmd {
    Refresh,
    SetTab { tab: String },
    SetFilter { filter: String },
    Search { text: String },
    /// Save the recipe on a web page.
    AddFromLink { url: String },
    /// New (id 0) or edited, from the editor's lines.
    SaveRecipe {
        id: i64,
        title: String,
        minutes: i64,
        servings: i64,
        category: String,
        ingredients: String,
        steps: String,
        source_url: String,
    },
    DeleteRecipe { id: i64 },
    Open { id: i64 },
    SetServings { servings: i64 },
    ToggleFavourite { id: i64 },
    /// In the kitchen or not, by ingredient key.
    SetStock { key: String, have: bool },
    /// The open recipe's missing ingredients, at the servings shown.
    AddMissing,
    StartCook,
    CookStep { delta: i64 },
    /// Last step done: counts as cooked.
    FinishCook,
    LeaveCook,
    /// recipe_id 0 and label "" clears the slot.
    SetMeal { day: String, meal: String, recipe_id: i64, label: String },
    ShiftWeek { delta: i64 },
    /// Empty dinners from today on, from favourites and what has not been
    /// cooked lately.
    FillGaps,
    /// The week's recipes, less the pantry, grouped by aisle.
    MakeList,
    ToggleItem { id: i64 },
    AddItem { text: String },
    RemoveItem { id: i64 },
    /// What is in the basket goes into the pantry and off the list.
    ClearBasket,
    /// The shop as an expense in Finances.
    LogShop { account_id: i64, amount: String },
    /// The week shown, as a calendar file the system calendar imports.
    ExportWeek,
    /// Something in the kitchen, typed: "2 tins of coconut milk".
    AddPantry { text: String },
    /// Start the pantry again: nothing is in stock.
    ClearPantry,
    /// Listen for spoken commands in cook mode, or stop.
    Listen { on: bool },
}

// ----------------------------------------------------------------- session ---

struct Session {
    tab: String,
    filter: String,
    query: String,
    open: i64,
    /// Servings chosen for the open recipe; None is the recipe's own.
    servings: Option<i64>,
    cook: Option<i64>,
    /// The Monday of the week on Plan; None follows the calendar.
    week: Option<NaiveDate>,
    list_from: i64,
    notice: String,
}

fn session() -> &'static Mutex<Session> {
    static S: OnceLock<Mutex<Session>> = OnceLock::new();
    S.get_or_init(|| {
        Mutex::new(Session {
            tab: "recipes".into(),
            filter: "all".into(),
            query: String::new(),
            open: 0,
            servings: None,
            cook: None,
            week: None,
            list_from: 0,
            notice: String::new(),
        })
    })
}

fn lock() -> MutexGuard<'static, Session> {
    match session().lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn say(notice: impl Into<String>) {
    lock().notice = notice.into();
}

/// The screen hold while cooking; dropping it lets the screen sleep again.
fn awake() -> &'static Mutex<Option<tokio::process::Child>> {
    static A: OnceLock<Mutex<Option<tokio::process::Child>>> = OnceLock::new();
    A.get_or_init(|| Mutex::new(None))
}

/// Move cook mode a step, within the recipe. Nothing when not cooking.
async fn step(pool: &SqlitePool, delta: i64) -> Result<()> {
    let id = lock().open;
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM steps WHERE recipe_id = ?").bind(id).fetch_one(pool).await?;
    let mut s = lock();
    if let Some(at) = s.cook {
        s.cook = Some((at + delta).clamp(0, (n - 1).max(0)));
    }
    Ok(())
}

// ── listening ───────────────────────────────────────────────────────────────
//
// Two and a half seconds at a time from the microphone, each clip through
// whisper-cli on this computer. One clip is transcribed while the next
// records; if whisper falls behind, a clip is dropped rather than queued, so
// what is heard is always recent. "next" and "back" move the step here; the
// page acts on "repeat", "timer" and "stop", which live in Dart.
//
// ponytail: whisper per clip is a second or two behind on a laptop CPU, and
// read-aloud can be heard by the microphone (only short utterances count, so
// a step's text does not). A keyword spotter would be quicker if that lag
// ever matters.

struct Listen {
    /// Bumped on every start and stop; a loop from an older start sees the
    /// change and ends.
    epoch: std::sync::atomic::AtomicU64,
    on: std::sync::atomic::AtomicBool,
    heard: Mutex<(String, i64)>,
}

static LISTEN: Listen = Listen {
    epoch: std::sync::atomic::AtomicU64::new(0),
    on: std::sync::atomic::AtomicBool::new(false),
    heard: Mutex::new((String::new(), 0)),
};

fn listening() -> bool {
    LISTEN.on.load(std::sync::atomic::Ordering::Acquire)
}

fn stop_listening() {
    use std::sync::atomic::Ordering;
    LISTEN.epoch.fetch_add(1, Ordering::AcqRel);
    LISTEN.on.store(false, Ordering::Release);
}

fn start_listening(pool: &'static SqlitePool) {
    use std::sync::atomic::Ordering;
    let epoch = LISTEN.epoch.fetch_add(1, Ordering::AcqRel) + 1;
    LISTEN.on.store(true, Ordering::Release);
    let alive = move || LISTEN.epoch.load(Ordering::Acquire) == epoch;
    let Some(dir) = tulipix_core::paths::cache_dir().map(|d| d.join("kitchen-listen")) else { return };
    std::fs::create_dir_all(&dir).ok();
    let (tx, mut rx) = tokio::sync::mpsc::channel::<std::path::PathBuf>(1);
    tokio::spawn(async move {
        while let Some(clip) = rx.recv().await {
            if !alive() {
                break;
            }
            let heard = crate::api::journal::transcribe(&clip).await.unwrap_or_default();
            if let Some(word) = k::command_word(&heard) {
                let delta = match word {
                    "next" => 1,
                    "back" => -1,
                    _ => 0,
                };
                if delta != 0 {
                    step(pool, delta).await.ok();
                }
                if let Ok(mut h) = LISTEN.heard.lock() {
                    *h = (word.to_string(), h.1 + 1);
                }
            }
        }
    });
    tokio::spawn(async move {
        let mut i = 0;
        while alive() {
            // Four names in turn: one being read, one waiting, one recording.
            let clip = dir.join(format!("{i}.wav"));
            i = (i + 1) % 4;
            if let Err(e) = crate::api::journal::record_clip(&clip, 2.5).await {
                tracing::info!(error = %e, "kitchen: listening stopped");
                if alive() {
                    stop_listening();
                    say(e.to_string());
                }
                break;
            }
            // Full: whisper is still on the last one, and this clip is dropped.
            let _ = tx.try_send(clip);
        }
    });
}

fn set_awake(on: bool) {
    if !on {
        stop_listening();
    }
    if let Ok(mut g) = awake().lock() {
        *g = if on { g.take().or_else(k::hold_awake) } else { None };
    }
}

fn today() -> NaiveDate {
    chrono::Local::now().date_naive()
}

fn monday(d: NaiveDate) -> NaiveDate {
    d - chrono::TimeDelta::days(d.weekday().num_days_from_monday() as i64)
}

// ---------------------------------------------------------------- exported ---

pub async fn kitchen_dispatch(cmd: KitchenCmd) -> Result<KitchenState> {
    let pool = kitchen_pool().await?;
    apply(pool, cmd).await?;
    snapshot(pool).await
}

/// A recipe as editor lines; id 0 is a blank one.
pub async fn kitchen_draft(id: i64) -> Result<RecipeDraft> {
    let pool = kitchen_pool().await?;
    if id == 0 {
        return Ok(RecipeDraft {
            id: 0,
            title: String::new(),
            minutes: 0,
            servings: 4,
            category: String::new(),
            ingredients: String::new(),
            steps: String::new(),
            source_url: String::new(),
        });
    }
    let (title, minutes, servings, category, source_url): (String, i64, i64, String, String) =
        sqlx::query_as("SELECT title, minutes, servings, category, source_url FROM recipes WHERE id = ?")
            .bind(id)
            .fetch_one(pool)
            .await?;
    let lines: Vec<String> = sqlx::query_scalar("SELECT line FROM ingredients WHERE recipe_id = ? ORDER BY pos")
        .bind(id)
        .fetch_all(pool)
        .await?;
    let steps: Vec<String> = sqlx::query_scalar("SELECT text FROM steps WHERE recipe_id = ? ORDER BY pos")
        .bind(id)
        .fetch_all(pool)
        .await?;
    Ok(RecipeDraft {
        id,
        title,
        minutes,
        servings,
        category,
        ingredients: lines.join("\n"),
        steps: steps.join("\n"),
        source_url,
    })
}

/// A cookbook page photographed, read by tesseract into an unsaved draft.
pub async fn kitchen_ocr(path: String) -> Result<RecipeDraft> {
    let d = tokio::task::spawn_blocking(move || k::ocr(std::path::Path::new(&path))).await??;
    Ok(RecipeDraft {
        id: 0,
        title: d.title,
        minutes: 0,
        servings: d.servings,
        category: String::new(),
        ingredients: d.ingredients.join("\n"),
        steps: d.steps.join("\n"),
        source_url: String::new(),
    })
}

/// The shopping list as a page a phone on the same network can open, and the
/// QR code that opens it. A new code each time: the old one stops working.
pub async fn kitchen_phone() -> Result<PhoneLink> {
    let url = k::phone::publish().await?;
    Ok(PhoneLink { qr: crate::api::transfer::render_qr(&url), url })
}

// ------------------------------------------------------------------- apply ---

async fn apply(pool: &'static SqlitePool, cmd: KitchenCmd) -> Result<()> {
    match cmd {
        KitchenCmd::Refresh => {}
        KitchenCmd::SetTab { tab } => lock().tab = tab,
        KitchenCmd::SetFilter { filter } => lock().filter = filter,
        KitchenCmd::Search { text } => {
            let mut s = lock();
            s.query = text.trim().to_string();
            s.tab = "recipes".into();
        }
        KitchenCmd::AddFromLink { url } => {
            let id = add_from_link(pool, &url).await?;
            open(id);
        }
        KitchenCmd::SaveRecipe { id, title, minutes, servings, category, ingredients, steps, source_url } => {
            if title.trim().is_empty() {
                bail!("a recipe needs a name");
            }
            let d = k::Draft {
                title: title.trim().to_string(),
                image: String::new(),
                minutes: minutes.max(0),
                servings: servings.clamp(1, 100),
                category: category.trim().to_string(),
                ingredients: lines(&ingredients),
                steps: lines(&steps),
            };
            let id = save(pool, id, &d, source_url.trim()).await?;
            open(id);
        }
        KitchenCmd::DeleteRecipe { id } => {
            let image: Option<String> = sqlx::query_scalar("SELECT image FROM recipes WHERE id = ?")
                .bind(id)
                .fetch_optional(pool)
                .await?;
            sqlx::query("DELETE FROM recipes WHERE id = ?").bind(id).execute(pool).await?;
            if let Some(p) = image.filter(|p| !p.is_empty()) {
                std::fs::remove_file(p).ok();
            }
            let mut s = lock();
            if s.open == id {
                s.open = 0;
                s.cook = None;
                s.tab = "recipes".into();
            }
        }
        KitchenCmd::Open { id } => open(id),
        KitchenCmd::SetServings { servings } => lock().servings = Some(servings.clamp(1, 100)),
        KitchenCmd::ToggleFavourite { id } => {
            sqlx::query("UPDATE recipes SET favourite = 1 - favourite WHERE id = ?").bind(id).execute(pool).await?;
        }
        KitchenCmd::SetStock { key, have } => {
            let sql = if have {
                "INSERT OR IGNORE INTO pantry (key) VALUES (?)"
            } else {
                "DELETE FROM pantry WHERE key = ?"
            };
            sqlx::query(sql).bind(key).execute(pool).await?;
        }
        KitchenCmd::AddMissing => {
            let id = lock().open;
            let Some(v) = recipe_view(pool, id).await? else { bail!("open a recipe first") };
            let factor = v.servings as f64 / v.base_servings.max(1) as f64;
            let rows = ingredient_rows(pool, id).await?;
            let have = pantry(pool).await?;
            let mut added = 0;
            for r in rows.iter().filter(|r| !pantry_has(&have, &r.key)) {
                if add_to_list(pool, &r.key, &r.name, r.qty.map(|q| q * factor), &r.unit, "recipe").await? {
                    added += 1;
                }
            }
            say(match added {
                0 => "Everything missing is already on the list".to_string(),
                n => format!("{} added to the shopping list", plural(n, "item", "items")),
            });
        }
        KitchenCmd::StartCook => {
            let mut s = lock();
            if s.open == 0 {
                bail!("open a recipe first");
            }
            s.cook = Some(0);
            s.tab = "cook".into();
            drop(s);
            set_awake(true);
        }
        KitchenCmd::CookStep { delta } => step(pool, delta).await?,
        KitchenCmd::FinishCook => {
            let id = lock().open;
            sqlx::query("UPDATE recipes SET cooked = cooked + 1, last_cooked = ? WHERE id = ?")
                .bind(k::now())
                .bind(id)
                .execute(pool)
                .await?;
            let times: i64 = sqlx::query_scalar("SELECT cooked FROM recipes WHERE id = ?").bind(id).fetch_one(pool).await?;
            let mut s = lock();
            s.cook = None;
            s.tab = "recipe".into();
            s.notice = format!("Enjoy it — cooked {}", plural(times, "time", "times"));
            drop(s);
            set_awake(false);
        }
        KitchenCmd::LeaveCook => {
            let mut s = lock();
            s.cook = None;
            s.tab = "recipe".into();
            drop(s);
            set_awake(false);
        }
        KitchenCmd::SetMeal { day, meal, recipe_id, label } => {
            if !matches!(meal.as_str(), "lunch" | "dinner") {
                bail!("a meal is lunch or dinner");
            }
            if recipe_id == 0 && label.trim().is_empty() {
                sqlx::query("DELETE FROM plan WHERE day = ? AND meal = ?").bind(day).bind(meal).execute(pool).await?;
            } else {
                sqlx::query(
                    "INSERT INTO plan (day, meal, recipe_id, label) VALUES (?, ?, ?, ?) \
                     ON CONFLICT (day, meal) DO UPDATE SET recipe_id = excluded.recipe_id, label = excluded.label",
                )
                .bind(day)
                .bind(meal)
                .bind((recipe_id != 0).then_some(recipe_id))
                .bind(label.trim())
                .execute(pool)
                .await?;
            }
        }
        KitchenCmd::ShiftWeek { delta } => {
            let mut s = lock();
            let start = s.week.unwrap_or_else(|| monday(today()));
            let next = start + chrono::TimeDelta::days(7 * delta);
            s.week = if next == monday(today()) { None } else { Some(next) };
        }
        KitchenCmd::FillGaps => {
            let start = lock().week.unwrap_or_else(|| monday(today()));
            let filled = fill_gaps(pool, start).await?;
            say(match filled {
                0 => "No empty dinners to fill".to_string(),
                n => format!("Filled {}", plural(n, "dinner", "dinners")),
            });
        }
        KitchenCmd::MakeList => {
            let start = lock().week.unwrap_or_else(|| monday(today()));
            let (meals, added) = make_list(pool, start).await?;
            let mut s = lock();
            s.list_from = meals;
            s.tab = "shop".into();
            s.notice = if meals == 0 {
                "Nothing planned this week has a recipe to shop for".into()
            } else {
                format!("{} from {}", plural(added, "item", "items"), plural(meals, "planned meal", "planned meals"))
            };
        }
        KitchenCmd::ExportWeek => {
            let start = lock().week.unwrap_or_else(|| monday(today()));
            let w = week_view(pool, start).await?;
            let mut events = Vec::new();
            for d in &w.days {
                let Some(day) = d.day.parse::<NaiveDate>().ok() else { continue };
                for (meal, slot) in [("Lunch", &d.lunch), ("Dinner", &d.dinner)] {
                    let what = if slot.title.is_empty() { &slot.label } else { &slot.title };
                    if what.trim().is_empty() {
                        continue;
                    }
                    events.push(crate::ics::Event {
                        uid: format!("meal-{}-{}", d.day, meal.to_lowercase()),
                        day,
                        title: format!("{meal}: {what}"),
                        note: String::new(),
                    });
                }
            }
            if events.is_empty() {
                bail!("nothing is planned that week");
            }
            crate::ics::open(&format!("meals-{}", w.start), &events)?;
            say(format!("Sent {} to your calendar", plural(events.len() as i64, "meal", "meals")));
        }
        KitchenCmd::AddPantry { text } => {
            let key = k::key(&k::parse_ingredient(&text).name);
            if key.is_empty() {
                bail!("what is in the kitchen?");
            }
            sqlx::query("INSERT OR IGNORE INTO pantry (key) VALUES (?)").bind(key).execute(pool).await?;
        }
        KitchenCmd::Listen { on } => {
            if on {
                if lock().cook.is_none() {
                    bail!("start cooking first");
                }
                crate::api::journal::transcribe_ready()?;
                start_listening(pool);
            } else {
                stop_listening();
            }
        }
        KitchenCmd::ClearPantry => {
            sqlx::query("DELETE FROM pantry").execute(pool).await?;
            say("The pantry is empty: every recipe will ask for everything");
        }
        KitchenCmd::ToggleItem { id } => {
            sqlx::query("UPDATE shop SET done = 1 - done WHERE id = ?").bind(id).execute(pool).await?;
        }
        KitchenCmd::AddItem { text } => {
            let i = k::parse_ingredient(&text);
            if i.name.trim().is_empty() {
                bail!("what should go on the list?");
            }
            let key = k::key(&i.name);
            add_to_list(pool, if key.is_empty() { &i.name } else { &key }, &i.name, i.qty, &i.unit, "typed").await?;
        }
        KitchenCmd::RemoveItem { id } => {
            sqlx::query("DELETE FROM shop WHERE id = ?").bind(id).execute(pool).await?;
        }
        KitchenCmd::ClearBasket => {
            sqlx::query("INSERT OR IGNORE INTO pantry (key) SELECT key FROM shop WHERE done = 1")
                .execute(pool)
                .await?;
            let gone = sqlx::query("DELETE FROM shop WHERE done = 1").execute(pool).await?.rows_affected();
            say(format!("{} put away", plural(gone as i64, "item", "items")));
        }
        KitchenCmd::LogShop { account_id, amount } => log_shop(account_id, &amount).await?,
    }
    Ok(())
}

fn open(id: i64) {
    let mut s = lock();
    if s.open != id {
        s.servings = None;
        s.cook = None;
    }
    s.open = id;
    s.tab = "recipe".into();
    s.query.clear();
}

fn lines(s: &str) -> Vec<String> {
    s.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect()
}

fn plural(n: i64, one: &str, many: &str) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{n} {many}") }
}

/// Write a recipe's row, ingredients and steps, replacing them when it
/// already exists. Returns its id.
async fn save(pool: &SqlitePool, id: i64, d: &k::Draft, source_url: &str) -> Result<i64> {
    let mut tx = pool.begin().await?;
    let id = if id == 0 {
        sqlx::query(
            "INSERT INTO recipes (title, source_url, minutes, servings, category, created) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&d.title)
        .bind(source_url)
        .bind(d.minutes)
        .bind(d.servings)
        .bind(&d.category)
        .bind(k::now())
        .execute(&mut *tx)
        .await?
        .last_insert_rowid()
    } else {
        sqlx::query("UPDATE recipes SET title = ?, source_url = ?, minutes = ?, servings = ?, category = ? WHERE id = ?")
            .bind(&d.title)
            .bind(source_url)
            .bind(d.minutes)
            .bind(d.servings)
            .bind(&d.category)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM ingredients WHERE recipe_id = ?").bind(id).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM steps WHERE recipe_id = ?").bind(id).execute(&mut *tx).await?;
        id
    };
    for (pos, line) in d.ingredients.iter().enumerate() {
        let i = k::parse_ingredient(line);
        let name = if i.name.is_empty() { line.clone() } else { i.name.clone() };
        sqlx::query(
            "INSERT INTO ingredients (recipe_id, pos, line, qty, unit, name, note, key) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(pos as i64)
        .bind(line)
        .bind(i.qty)
        .bind(&i.unit)
        .bind(&name)
        .bind(&i.note)
        .bind(k::key(&name))
        .execute(&mut *tx)
        .await?;
    }
    for (pos, text) in d.steps.iter().enumerate() {
        sqlx::query("INSERT INTO steps (recipe_id, pos, text) VALUES (?, ?, ?)")
            .bind(id)
            .bind(pos as i64)
            .bind(text)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(id)
}

async fn add_from_link(pool: &SqlitePool, input: &str) -> Result<i64> {
    let mut url = input.trim().to_string();
    if !url.contains("://") {
        url = format!("https://{url}");
    }
    if !crate::feeds::is_web(&url) {
        bail!("only http and https addresses can be read");
    }
    let client = crate::feeds::client();
    let body = client.get(&url).send().await?.error_for_status()?.bytes().await?;
    if body.len() > 12 * 1024 * 1024 {
        bail!("that page is too big to be a recipe");
    }
    let html = String::from_utf8_lossy(&body);
    let d = k::extract(&html)
        .ok_or_else(|| anyhow!("that page has no recipe data to read — add it by hand with New recipe"))?;
    let id = save(pool, 0, &d, &url).await?;
    if !d.image.is_empty() {
        let base = reqwest::Url::parse(&url)?;
        if let Ok(img) = base.join(&d.image) {
            match fetch_image(&client, img.as_str(), id).await {
                Ok(path) => {
                    sqlx::query("UPDATE recipes SET image = ? WHERE id = ?").bind(path).bind(id).execute(pool).await?;
                }
                Err(e) => tracing::info!(error = %e, "kitchen: kept the recipe; its photo did not load"),
            }
        }
    }
    say(format!("Saved {}", d.title));
    Ok(id)
}

/// The recipe's photo, kept on disk so the card draws offline.
async fn fetch_image(client: &reqwest::Client, url: &str, id: i64) -> Result<String> {
    let resp = client.get(url).send().await?.error_for_status()?;
    let ext = match resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("") {
        t if t.contains("png") => "png",
        t if t.contains("webp") => "webp",
        _ => "jpg",
    };
    let bytes = resp.bytes().await?;
    if bytes.len() > 20 * 1024 * 1024 {
        bail!("photo too big");
    }
    let dir = tulipix_core::paths::data_dir().ok_or_else(|| anyhow!("no data folder"))?.join("kitchen").join("images");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{id}.{ext}"));
    tokio::fs::write(&path, &bytes).await?;
    Ok(path.to_string_lossy().to_string())
}

async fn pantry(pool: &SqlitePool) -> Result<HashSet<String>> {
    Ok(sqlx::query_scalar::<_, String>("SELECT key FROM pantry").fetch_all(pool).await?.into_iter().collect())
}

fn pantry_has(p: &HashSet<String>, key: &str) -> bool {
    key.is_empty() || k::staple(key) || p.contains(key)
}

/// Put a line on the list unless its key is already there. True when added.
async fn add_to_list(pool: &SqlitePool, key: &str, name: &str, qty: Option<f64>, unit: &str, origin: &str) -> Result<bool> {
    let there: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM shop WHERE key = ?").bind(key).fetch_one(pool).await?;
    if there > 0 {
        return Ok(false);
    }
    sqlx::query("INSERT INTO shop (key, name, qty, unit, aisle, origin, created) VALUES (?, ?, ?, ?, ?, ?, ?)")
        .bind(key)
        .bind(capital(name))
        .bind(qty)
        .bind(unit)
        .bind(k::aisle(key))
        .bind(origin)
        .bind(k::now())
        .execute(pool)
        .await?;
    Ok(true)
}

fn capital(s: &str) -> String {
    let mut c = s.trim().chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

struct IngLine {
    id: i64,
    qty: Option<f64>,
    unit: String,
    name: String,
    note: String,
    key: String,
}

async fn ingredient_rows(pool: &SqlitePool, recipe: i64) -> Result<Vec<IngLine>> {
    let rows: Vec<(i64, Option<f64>, String, String, String, String)> = sqlx::query_as(
        "SELECT id, qty, unit, name, note, key FROM ingredients WHERE recipe_id = ? ORDER BY pos",
    )
    .bind(recipe)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id, qty, unit, name, note, key)| IngLine { id, qty, unit, name, note, key }).collect())
}

async fn fill_gaps(pool: &SqlitePool, start: NaiveDate) -> Result<i64> {
    let from = start.max(today());
    let end = start + chrono::TimeDelta::days(7);
    let taken: HashSet<i64> = sqlx::query_scalar::<_, i64>(
        "SELECT recipe_id FROM plan WHERE day >= ? AND day < ? AND recipe_id IS NOT NULL",
    )
    .bind(start.to_string())
    .bind(end.to_string())
    .fetch_all(pool)
    .await?
    .into_iter()
    .collect();
    let mut pool_of: Vec<i64> = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM recipes ORDER BY favourite DESC, last_cooked ASC, id",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .filter(|id| !taken.contains(id))
    .collect();
    pool_of.reverse();
    let mut filled = 0;
    let mut d = from;
    while d < end {
        let day = d.to_string();
        let busy: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM plan WHERE day = ? AND meal = 'dinner'")
            .bind(&day)
            .fetch_one(pool)
            .await?;
        if busy == 0 {
            let Some(id) = pool_of.pop() else { break };
            sqlx::query("INSERT INTO plan (day, meal, recipe_id) VALUES (?, 'dinner', ?)")
                .bind(&day)
                .bind(id)
                .execute(pool)
                .await?;
            filled += 1;
        }
        d += chrono::TimeDelta::days(1);
    }
    Ok(filled)
}

/// Remake the plan's part of the list for a week: every planned recipe's
/// ingredients, summed like for like, less what is in the pantry and what is
/// already on the list. Returns (planned meals, lines added).
async fn make_list(pool: &SqlitePool, start: NaiveDate) -> Result<(i64, i64)> {
    let end = start + chrono::TimeDelta::days(7);
    let recipes: Vec<i64> = sqlx::query_scalar(
        "SELECT recipe_id FROM plan WHERE day >= ? AND day < ? AND recipe_id IS NOT NULL",
    )
    .bind(start.to_string())
    .bind(end.to_string())
    .fetch_all(pool)
    .await?;
    sqlx::query("DELETE FROM shop WHERE origin = 'plan' AND done = 0").execute(pool).await?;
    let have = pantry(pool).await?;
    let mut lines = Vec::new();
    for id in &recipes {
        for r in ingredient_rows(pool, *id).await? {
            if !pantry_has(&have, &r.key) {
                lines.push((r.key, r.name, r.qty, r.unit));
            }
        }
    }
    let mut added = 0;
    for (key, name, qty, unit) in k::merge(&lines) {
        if add_to_list(pool, &key, &name, qty, &unit, "plan").await? {
            added += 1;
        }
    }
    Ok((recipes.len() as i64, added))
}

async fn log_shop(account_id: i64, amount: &str) -> Result<()> {
    use tulipix_finances::{accounts, fx, money, txn};
    let pool = crate::db::finances_pool().await?;
    let acct = accounts::list(pool, false)
        .await?
        .into_iter()
        .find(|a| a.id == account_id)
        .ok_or_else(|| anyhow!("that account is not in Finances any more"))?;
    let minor = money::parse_amount(amount.trim(), &acct.currency)?;
    if minor <= 0 {
        bail!("the amount should be more than nothing");
    }
    let base = fx::base_currency();
    let mut t = txn::NewTxn::expense(acct.id, minor, &acct.currency, &today().to_string(), "Food shop");
    t.rate_micro = fx::rate_for(pool, &acct.currency, &base).await?;
    t.category_id = sqlx::query_scalar(
        "SELECT id FROM categories WHERE kind = 'expense' AND lower(name) IN ('groceries', 'food', 'food & groceries') \
         ORDER BY parent_id IS NOT NULL LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;
    t.note = Some("From Kitchen's shopping list".into());
    txn::post(pool, &t).await?;
    say(format!("Logged {} in Finances", money::format_minor(minor, &acct.currency)));
    Ok(())
}

// ---------------------------------------------------------------- snapshot ---

fn time_label(minutes: i64) -> String {
    match (minutes / 60, minutes % 60) {
        (0, 0) => String::new(),
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} m"),
    }
}

fn host(url: &str) -> String {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.trim_start_matches("www.").to_string()))
        .unwrap_or_default()
}

async fn recipe_view(pool: &SqlitePool, id: i64) -> Result<Option<RecipeView>> {
    let row: Option<(String, String, String, i64, i64, String, bool, i64)> = sqlx::query_as(
        "SELECT title, source_url, image, minutes, servings, category, favourite, cooked FROM recipes WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    let Some((title, url, image, minutes, base, category, favourite, cooked)) = row else { return Ok(None) };
    let servings = lock().servings.unwrap_or(base);
    let factor = servings as f64 / base.max(1) as f64;
    let have = pantry(pool).await?;
    let ings = ingredient_rows(pool, id).await?;
    let ingredients: Vec<IngRow> = ings
        .iter()
        .map(|r| IngRow {
            id: r.id,
            amount: k::amount(r.qty, &r.unit, factor),
            name: r.name.clone(),
            note: r.note.clone(),
            key: r.key.clone(),
            in_stock: pantry_has(&have, &r.key),
            staple: k::staple(&r.key),
        })
        .collect();
    let names: Vec<String> = ings.iter().map(|r| r.name.clone()).collect();
    let texts: Vec<String> = sqlx::query_scalar("SELECT text FROM steps WHERE recipe_id = ? ORDER BY pos")
        .bind(id)
        .fetch_all(pool)
        .await?;
    let steps = texts
        .into_iter()
        .map(|text| StepRow {
            timers: k::timers(&text).into_iter().map(|(label, secs)| TimerSpec { label, secs }).collect(),
            uses: k::uses(&text, &names)
                .into_iter()
                .map(|i| {
                    let a = &ingredients[i].amount;
                    let n = ingredients[i].name.to_lowercase();
                    if a.is_empty() { n } else { format!("{a} {n}") }
                })
                .collect(),
            text,
        })
        .collect();
    Ok(Some(RecipeView {
        missing: ingredients.iter().filter(|i| !i.in_stock).count() as i64,
        id,
        title,
        host: host(&url),
        url,
        image,
        time: time_label(minutes),
        minutes,
        category,
        cooked,
        favourite,
        servings,
        base_servings: base,
        ingredients,
        steps,
    }))
}

async fn snapshot(pool: &SqlitePool) -> Result<KitchenState> {
    let (tab, filter, query, open, cook, week, list_from, notice) = {
        let mut s = lock();
        (
            s.tab.clone(),
            s.filter.clone(),
            s.query.clone(),
            s.open,
            s.cook,
            s.week.unwrap_or_else(|| monday(today())),
            s.list_from,
            std::mem::take(&mut s.notice),
        )
    };

    // Every card, then what the filters count and keep.
    let rows: Vec<(i64, String, String, i64, String, bool)> =
        sqlx::query_as("SELECT id, title, image, minutes, category, favourite FROM recipes ORDER BY title COLLATE NOCASE")
            .fetch_all(pool)
            .await?;
    let have = pantry(pool).await?;
    let mut keys: HashMap<i64, Vec<String>> = HashMap::new();
    for (rid, key) in sqlx::query_as::<_, (i64, String)>("SELECT recipe_id, key FROM ingredients").fetch_all(pool).await? {
        keys.entry(rid).or_default().push(key);
    }
    let q = query.to_lowercase();
    let matching: HashSet<i64> = if q.is_empty() {
        HashSet::new()
    } else {
        sqlx::query_scalar::<_, i64>(
            "SELECT id FROM recipes WHERE instr(lower(title), ?1) > 0 \
             UNION SELECT recipe_id FROM ingredients WHERE instr(lower(name), ?1) > 0",
        )
        .bind(&q)
        .fetch_all(pool)
        .await?
        .into_iter()
        .collect()
    };
    let cards: Vec<RecipeCard> = rows
        .into_iter()
        .map(|(id, title, image, minutes, category, favourite)| RecipeCard {
            can_make: keys.get(&id).is_some_and(|ks| !ks.is_empty() && ks.iter().all(|k| pantry_has(&have, k))),
            time: time_label(minutes),
            id,
            title,
            image,
            category,
            favourite,
        })
        .collect();
    let minutes_of: HashMap<i64, i64> =
        sqlx::query_as::<_, (i64, i64)>("SELECT id, minutes FROM recipes").fetch_all(pool).await?.into_iter().collect();
    let keep = |f: &str, c: &RecipeCard| -> bool {
        let cat = c.category.to_lowercase();
        match f {
            "fav" => c.favourite,
            "quick" => minutes_of.get(&c.id).is_some_and(|m| *m > 0 && *m <= 30),
            "veg" => cat == "vegetarian" || cat == "vegan",
            "baking" => cat == "baking" || cat == "dessert",
            "canmake" => c.can_make,
            _ => true,
        }
    };
    let filters: Vec<FilterChip> = [
        ("all", "All"),
        ("fav", "Favourites"),
        ("quick", "Quick"),
        ("veg", "Vegetarian"),
        ("baking", "Baking"),
        ("canmake", "Can make now"),
    ]
    .iter()
    .map(|&(id, label)| FilterChip {
        id: id.to_string(),
        label: label.to_string(),
        n: cards.iter().filter(|c| keep(id, *c)).count() as i64,
    })
    .collect();
    let total = cards.len() as i64;
    let recipes: Vec<RecipeCard> = cards
        .into_iter()
        .filter(|c| keep(filter.as_str(), c) && (q.is_empty() || matching.contains(&c.id)))
        .collect();
    let picks: Vec<RecipePick> = if tab == "plan" {
        sqlx::query_as::<_, (i64, String)>("SELECT id, title FROM recipes ORDER BY title COLLATE NOCASE")
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|(id, title)| RecipePick { id, title })
            .collect()
    } else {
        Vec::new()
    };

    let open_view = if open != 0 { recipe_view(pool, open).await? } else { None };
    let cook_step = cook.unwrap_or(0);

    let shop_left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM shop WHERE done = 0").fetch_one(pool).await?;
    let accounts = if tab == "shop" {
        match crate::db::finances_pool().await {
            Ok(fp) => tulipix_finances::accounts::list(fp, false)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|a| AccountPick { id: a.id, name: a.name, currency: a.currency })
                .collect(),
            Err(_) => Vec::new(),
        }
    } else {
        Vec::new()
    };

    let mut pantry_groups: Vec<PantryGroup> =
        AISLES.iter().map(|a| PantryGroup { aisle: a.to_string(), keys: Vec::new() }).collect();
    if tab == "pantry" {
        let mut sorted: Vec<&String> = have.iter().collect();
        sorted.sort();
        for key in sorted {
            let a = k::aisle(key);
            if let Some(g) = pantry_groups.iter_mut().find(|g| g.aisle == a) {
                g.keys.push(key.clone());
            }
        }
    }
    pantry_groups.retain(|g| !g.keys.is_empty());

    let (heard, heard_n) = LISTEN.heard.lock().map(|h| h.clone()).unwrap_or_default();
    Ok(KitchenState {
        listening: listening(),
        heard,
        heard_n,
        pantry: pantry_groups,
        pantry_n: have.len() as i64,
        cooking: cook.is_some() && open_view.is_some(),
        cook_step,
        open: open_view,
        week: if tab == "plan" { week_view(pool, week).await? } else { WeekView::default() },
        shop: if tab == "shop" { shop_view(pool, list_from).await? } else { ShopView::default() },
        shop_left,
        accounts,
        picks,
        filters,
        recipes,
        total,
        filter,
        query,
        notice,
        tab,
    })
}

async fn week_view(pool: &SqlitePool, start: NaiveDate) -> Result<WeekView> {
    let end = start + chrono::TimeDelta::days(6);
    let rows: Vec<(String, String, Option<i64>, String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT p.day, p.meal, p.recipe_id, p.label, r.title, r.image FROM plan p \
         LEFT JOIN recipes r ON r.id = p.recipe_id WHERE p.day >= ? AND p.day <= ?",
    )
    .bind(start.to_string())
    .bind(end.to_string())
    .fetch_all(pool)
    .await?;
    let mut slots: HashMap<(String, String), PlanSlot> = HashMap::new();
    for (day, meal, rid, label, title, image) in rows {
        slots.insert(
            (day, meal),
            PlanSlot {
                recipe_id: rid.unwrap_or(0),
                title: title.unwrap_or_default(),
                image: image.unwrap_or_default(),
                label,
            },
        );
    }
    let planned = slots.len() as i64;
    let t = today();
    let this = monday(t);
    let days = (0..7)
        .map(|i| {
            let d = start + chrono::TimeDelta::days(i);
            let iso = d.to_string();
            PlanDay {
                dow: d.format("%a").to_string(),
                n: d.day() as i64,
                today: d == t,
                past: d < t,
                lunch: slots.remove(&(iso.clone(), "lunch".to_string())).unwrap_or_default(),
                dinner: slots.remove(&(iso.clone(), "dinner".to_string())).unwrap_or_default(),
                day: iso,
            }
        })
        .collect();
    Ok(WeekView {
        start: start.to_string(),
        title: if start == this {
            "This week".into()
        } else if start == this + chrono::TimeDelta::days(7) {
            "Next week".into()
        } else if start == this - chrono::TimeDelta::days(7) {
            "Last week".into()
        } else {
            format!("Week of {}", start.format("%-d %B"))
        },
        range: if start.month() == end.month() {
            format!("{}–{} {}", start.day(), end.day(), end.format("%B"))
        } else {
            format!("{} – {}", start.format("%-d %b"), end.format("%-d %b"))
        },
        planned,
        days,
    })
}

async fn shop_view(pool: &SqlitePool, list_from: i64) -> Result<ShopView> {
    let rows: Vec<(i64, String, Option<f64>, String, String, bool)> =
        sqlx::query_as("SELECT id, name, qty, unit, aisle, done FROM shop ORDER BY name COLLATE NOCASE")
            .fetch_all(pool)
            .await?;
    let total = rows.len() as i64;
    let in_basket = rows.iter().filter(|r| r.5).count() as i64;
    let mut groups: Vec<ShopGroup> = AISLES
        .iter()
        .map(|a| ShopGroup { aisle: a.to_string(), items: Vec::new() })
        .collect();
    for (id, name, qty, unit, aisle, done) in rows {
        let at = groups.iter().position(|g| g.aisle == aisle).unwrap_or(groups.len() - 1);
        groups[at].items.push(ShopItem { id, name, amount: k::amount(qty, &unit, 1.0), done });
    }
    groups.retain(|g| !g.items.is_empty());
    Ok(ShopView { from_meals: list_from, in_basket, total, groups })
}

// ------------------------------------------------------------------- phone ---

/// The shopping list as the phone's page — `kitchen::phone` serves it.
pub(crate) async fn phone_page() -> Result<String> {
    let v = shop_view(kitchen_pool().await?, 0).await?;
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;");
    let mut h = String::from(
        "<!doctype html><meta name=viewport content='width=device-width,initial-scale=1'>\
         <title>Shopping list</title><style>\
         body{font:17px system-ui,sans-serif;margin:0 16px 40px;color:#1a1a1a;background:#fbfaf5}\
         h1{font-size:22px;margin:20px 0 4px}h2{font-size:14px;color:#6b6b6b;margin:22px 0 6px;text-transform:uppercase}\
         label{display:flex;gap:12px;align-items:center;padding:12px 0;border-bottom:1px solid #e8e6dc}\
         input{width:22px;height:22px;accent-color:#84cc16}span.q{margin-left:auto;color:#777}\
         input:checked+span{text-decoration:line-through;color:#999}</style><h1>Shopping list</h1>",
    );
    if v.groups.is_empty() {
        h.push_str("<p>Nothing on the list.</p>");
    }
    for g in &v.groups {
        h.push_str(&format!("<h2>{}</h2>", esc(&g.aisle)));
        for i in &g.items {
            h.push_str(&format!(
                "<label><input type=checkbox data-id={}{}><span>{}</span><span class=q>{}</span></label>",
                i.id,
                if i.done { " checked" } else { "" },
                esc(&i.name),
                esc(&i.amount)
            ));
        }
    }
    // Each tick goes back to the computer, so the basket there is the basket
    // here. A failed post unticks the box rather than pretending it went.
    h.push_str(
        "<script>document.querySelectorAll('input[data-id]').forEach(b=>b.onchange=()=>{\
         fetch(location.pathname+'/tick/'+b.dataset.id+'/'+(b.checked?1:0),{method:'POST'})\
         .then(r=>{if(!r.ok)throw 0}).catch(()=>{b.checked=!b.checked})})</script>",
    );
    Ok(h)
}

/// A tick from the phone page: in the basket, or back out.
pub(crate) async fn phone_tick(id: i64, done: bool) -> Result<()> {
    let n = sqlx::query("UPDATE shop SET done = ? WHERE id = ?")
        .bind(done)
        .bind(id)
        .execute(kitchen_pool().await?)
        .await?
        .rows_affected();
    if n == 0 {
        bail!("that item is not on the list any more");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_read_the_way_the_deck_prints_them() {
        assert_eq!(time_label(35), "35 min");
        assert_eq!(time_label(90), "1 h 30 m");
        assert_eq!(time_label(1440), "24 h");
        assert_eq!(time_label(0), "");
    }

    #[test]
    fn weeks_start_on_monday() {
        let d = NaiveDate::from_ymd_opt(2026, 9, 19).unwrap(); // a Saturday
        assert_eq!(monday(d), NaiveDate::from_ymd_opt(2026, 9, 14).unwrap());
        assert_eq!(monday(monday(d)), monday(d));
    }
}
