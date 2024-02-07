use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat;
use chrono::{DateTime, Local, Utc};
use clap::{Parser, Subcommand};
use futures_util::stream::StreamExt;
use grammers_client::{Client, Config, SignInError};
use grammers_session::Session;
use log;
use partial_sort::PartialSort;
use simple_logger::SimpleLogger;
use std::fs;
use std::io::{self, BufRead as _, Write as _};
use std::path::PathBuf;
use tera::Tera;
use tokio::runtime;
use tokio::time::sleep;
use tokio::time::Duration;

// Trait for extending std::path::PathBuf
use path_slash::PathBufExt as _;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const SESSION_FILE: &str = "tgdigest.session";

#[derive(Copy, Clone)]
enum ActionType {
    Replies = 0,
    Reactions,
    Forwards,
    Views,
}

impl ActionType {
    fn from(value: usize) -> ActionType {
        match value {
            0 => ActionType::Replies,
            1 => ActionType::Reactions,
            2 => ActionType::Forwards,
            3 => ActionType::Views,
            _ => panic!("No ActionType for {value}"),
        }
    }
}

#[derive(Copy, Clone, serde::Serialize)]
pub struct Post {
    date: i64,
    id: i32,
    views: Option<i32>,
    forwards: Option<i32>,
    replies: Option<i32>,
    reactions: Option<i32>,
}

impl Post {
    async fn get_by_date(
        messages: &mut grammers_client::client::messages::MessageIter,
        from_date: DateTime<Utc>,
        to_date: DateTime<Utc>,
    ) -> Result<Vec<Post>> {
        let mut posts: Vec<Post> = Vec::new();
        while let Some(message) = messages.next().await? {
            let message: grammers_client::types::Message = message;

            let date = DateTime::<Utc>::from_timestamp(message.date().timestamp(), 0).unwrap();
            if date > to_date {
                continue;
            }
            if date < from_date {
                break;
            }

            // let text = message.text().substring(0, 21);
            posts.push(Post {
                date: date.timestamp(),
                id: message.id(),
                views: message.view_count(),
                forwards: message.forward_count(),
                replies: message.reply_count(),
                reactions: message.reaction_count(),
            });
        }

        Result::Ok(posts)
    }

    fn count(&self, index: ActionType) -> Option<i32> {
        match index {
            ActionType::Replies => self.replies,
            ActionType::Reactions => self.reactions,
            ActionType::Forwards => self.forwards,
            ActionType::Views => self.views,
        }
    }
}

#[derive(serde::Serialize)]
struct TopPost {
    top_count: usize,
    replies: Vec<Post>,
    reactions: Vec<Post>,
    forwards: Vec<Post>,
    views: Vec<Post>,
}

impl TopPost {
    fn get_top_by(top_count: usize, posts: &mut Vec<Post>, action: ActionType) -> Vec<Post> {
        if posts.len() < top_count {
            panic!("Size of posts less than {}", top_count)
        }

        posts.partial_sort(top_count, |a, b| b.count(action).cmp(&a.count(action)));
        posts[0..top_count].to_vec()
    }

    fn get_top(top_count: usize, posts: &mut Vec<Post>) -> TopPost {
        TopPost {
            top_count,
            replies: Self::get_top_by(top_count, posts, ActionType::Replies),
            reactions: Self::get_top_by(top_count, posts, ActionType::Reactions),
            forwards: Self::get_top_by(top_count, posts, ActionType::Forwards),
            views: Self::get_top_by(top_count, posts, ActionType::Views),
        }
    }

    fn index(&self, index: ActionType) -> &Vec<Post> {
        match index {
            ActionType::Replies => &self.replies,
            ActionType::Reactions => &self.reactions,
            ActionType::Forwards => &self.forwards,
            ActionType::Views => &self.views,
        }
    }

    fn print(&self) {
        let headers = [
            format!("Top {} by comments:", self.top_count),
            format!("Top {} by reactions:", self.top_count),
            format!("Top {} by forwards:", self.top_count),
            format!("Top {} by views:", self.top_count),
        ];
        for (index, header) in headers.iter().enumerate() {
            println!("{header}");
            let action = ActionType::from(index);
            for (pos, post) in self.index(action).iter().enumerate() {
                match post.count(action) {
                    Some(count) => {
                        println!(
                            "\t{}. {}: {}\t({})",
                            pos + 1,
                            post.id,
                            count,
                            DateTime::<Utc>::from_timestamp(post.date, 0).unwrap()
                        );
                    }
                    None => {
                        println!("No data");
                        break;
                    }
                }
            }
            println!("");
        }
    }
}

#[derive(Clone, serde::Serialize)]
struct Card {
    id: i32,
    count: Option<i32>,
    header: String,
    icon: String,
    filter: String,
}

impl Card {
    fn default() -> Self {
        Card {
            id: -1,
            count: None,
            header: String::from("UNDEFINED"),
            icon: icon_url("⚠️"),
            filter: String::from(""),
        }
    }

    fn create_card(post: &Post, action: ActionType) -> Card {
        Card {
            id: post.id,
            count: post.count(action),
            ..Card::default()
        }
    }

    fn create_cards(posts: &Vec<Post>, action: ActionType) -> Option<Vec<Card>> {
        match posts
            .iter()
            .map(|p| Card::create_card(p, action))
            .filter(|c| c.count.is_some())
            .collect::<Vec<Card>>()
        {
            cards if !cards.is_empty() => Some(cards),
            _ => None,
        }
    }
}

#[derive(Clone, serde::Serialize)]
struct Block {
    header: String,
    icon: String,
    filter: String,
    cards: Option<Vec<Card>>,
}

impl Block {
    fn default() -> Self {
        Block {
            header: String::from("UNDEFINED"),
            icon: icon_url("⚠️"),
            filter: String::from(""),
            cards: None,
        }
    }
}

#[derive(Parser)]
#[command(name = "tgdigest")]
#[command(author = "Anton Sosnin <antsosnin@yandex.ru>")]
#[command(version = "0.5")]
#[command(about = "Create digest for your telegram channel", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// t.me/<CHANNEL_NAME>
    channel_name: String,

    /// Path to configuration file
    #[arg(short, long)]
    config: Option<std::path::PathBuf>,

    #[arg(long, default_value_t = 3)]
    /// Count of posts in digest
    top_count: usize,

    /// Template name from file-configured 'input_dir'
    #[arg(short, long)]
    mode: String,

    #[arg(short, long, default_value_t = -1)]
    /// The id of the post to place it in "Editor choice" block
    editor_choice_post_id: i32,

    #[arg(short, long)]
    from_date: Option<DateTime<Utc>>,

    #[arg(short, long)]
    to_date: Option<DateTime<Utc>>,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate cards from chosen digest posts from 1 to <TOP_COUNT>
    Cards {
        replies: usize,
        reactions: usize,
        forwards: usize,
        views: usize,
    },

    /// Generate digest
    Digest {},
}

fn icon_url(icon: &str) -> String {
    format!(
        "https://raw.githubusercontent.com/googlefonts/noto-emoji/main/svg/emoji_u{}.svg",
        format!("{:04x}", icon.chars().nth(0).unwrap_or('❌') as u32)
    )
}

fn prompt(message: &str) -> Result<String> {
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    stdout.write_all(message.as_bytes())?;
    stdout.flush()?;

    let stdin = io::stdin();
    let mut stdin = stdin.lock();

    let mut line = String::new();
    stdin.read_line(&mut line)?;
    Ok(line)
}

#[derive(Debug, serde::Deserialize)]
struct AppContext {
    input_dir: std::path::PathBuf,
    output_dir: std::path::PathBuf,
    session_file: std::path::PathBuf,
}

struct TelegramAPI {
    client: grammers_client::client::Client,
}

impl TelegramAPI {
    async fn create(ctx: &AppContext) -> Result<TelegramAPI> {
        let api_id: i32 = std::env::var("TG_ID")
            .expect(
                "TG_ID env var is not set. Visit https://my.telegram.org/ if you don't have it.",
            )
            .parse()
            .expect("TG_ID is not i32");
        let api_hash = std::env::var("TG_HASH").expect(
            "TG_HASH env var is not set. Visit https://my.telegram.org/ if you don't have it.",
        );

        println!("Connecting to Telegram...");
        let session_file = ctx.input_dir.join(SESSION_FILE);
        let session = match Session::load_file_or_create(&session_file) {
            Ok(session) => session,
            Err(why) => panic!(
                "Can't load session file {}: {why}",
                session_file.canonicalize().unwrap().to_str().unwrap()
            ),
        };
        let client = Client::connect(Config {
            session,
            api_id,
            api_hash,
            params: Default::default(),
        })
        .await
        .expect("Can't connect to Telegram");
        println!("Connected!");

        if !client.is_authorized().await.expect("Authorization error") {
            println!("Signing in...");
            let phone = prompt("Enter your phone number (international format): ")?;
            let token = client.request_login_code(&phone).await?;
            let code = prompt("Enter the code you received: ")?;
            let signed_in = client.sign_in(&token, &code).await;
            match signed_in {
                Err(SignInError::PasswordRequired(password_token)) => {
                    // Note: this `prompt` method will echo the password in the console.
                    //       Real code might want to use a better way to handle this.
                    let hint = password_token.hint().unwrap_or("None");
                    let prompt_message = format!("Enter the password (hint {}): ", &hint);
                    let password = prompt(prompt_message.as_str())?;

                    client
                        .check_password(password_token, password.trim())
                        .await?;
                }
                Ok(_) => (),
                Err(e) => panic!("{}", e),
            };
            println!("Signed in!");
            match client
                .session()
                .save_to_file(ctx.input_dir.join(SESSION_FILE))
            {
                Ok(_) => {}
                Err(e) => {
                    println!("NOTE: failed to save the session {}", e);
                }
            }
        }

        Ok(TelegramAPI { client })
    }

    fn client(&self) -> grammers_client::client::Client {
        // This handle can be `clone()`'d around and freely moved into other tasks, so you can invoke
        // methods concurrently if you need to. While you do this, the single owned `client` is the
        // one that communicates with the network.
        self.client.clone()
    }
}

fn fix_path_slash(path: &std::path::PathBuf) -> Result<std::path::PathBuf> {
    match path.to_slash() {
        Some(slashed) => Ok(std::path::PathBuf::from(slashed.to_string())),
        _ => Err(format!(
            "Can't handle the path '{}'",
            path.to_str().unwrap_or("<not UTF-8 path>")
        )
        .into()),
    }
}

fn handle_path(
    input: Option<std::path::PathBuf>,
    working_dir: &std::path::PathBuf,
) -> Result<std::path::PathBuf> {
    if working_dir.is_relative() {
        return Err(format!(
            "Working directory is not absolute: {}",
            working_dir.to_str().unwrap_or("<not UTF-8 path>")
        )
        .into());
    }

    let path = match input {
        Some(path) => working_dir.join(path),
        _ => working_dir.clone(),
    };

    if !path.exists() {
        return Err(format!(
            "Path does not exist: {}",
            path.to_str().unwrap_or("<not UTF-8 path>")
        )
        .into());
    }

    fix_path_slash(&path)
}

async fn async_main() -> Result<()> {
    let cli = Cli::parse();

    let current_date = DateTime::<Utc>::from_timestamp(Local::now().timestamp(), 0).unwrap();
    let week_ago = current_date - chrono::Duration::days(7);

    let from_date = cli.from_date.unwrap_or(week_ago);
    let to_date = cli.to_date.unwrap_or(current_date);

    let working_dir = std::env::current_dir()?;

    let config_path = handle_path(
        Some(cli.config.unwrap_or(PathBuf::from("./cfg.json"))),
        &working_dir,
    )?;
    println!(
        "Loading config: {}",
        config_path.to_str().expect("Can't find config file")
    );

    let data = fs::read_to_string(config_path).expect("Unable to read file");
    let ctx: AppContext = serde_json::from_str(data.as_str()).expect("Unable to parse cfg.json");
    let ctx: AppContext = AppContext {
        input_dir: handle_path(Some(ctx.input_dir), &working_dir)?,
        output_dir: handle_path(Some(ctx.output_dir), &working_dir)?,
        session_file: handle_path(Some(ctx.session_file), &working_dir)?,
    };
    println!("Loaded context {:#?}", ctx);

    SimpleLogger::new()
        .with_level(log::LevelFilter::Debug)
        .init()
        .unwrap();

    let tg = TelegramAPI::create(&ctx).await?;
    let client = tg.client();

    // LOAD CHAT
    let ithueti: grammers_client::types::chat::Chat = client
        .resolve_username(cli.channel_name.as_str())
        .await?
        .unwrap();
    let photo = ithueti.photo_downloadable(true);
    match photo {
        Some(photo) => {
            let photo_out = ctx.output_dir.join("pic.png");
            println!("Pic {}", photo_out.to_str().unwrap());
            client.download_media(&photo, photo_out).await?;
        }
        _ => {}
    }

    // GET MESSAGES
    let mut messages = client
        .iter_messages(ithueti)
        .max_date(to_date.timestamp() as i32)
        .limit(30000);
    let mut posts = Post::get_by_date(&mut messages, from_date, to_date).await?;

    let post_top = TopPost::get_top(cli.top_count, &mut posts);
    println!(
        "Fetched data for https://t.me/{} from {from_date} to {to_date}",
        cli.channel_name
    );

    post_top.print();

    // Template part

    let mut tera = Tera::default();

    let digest_template = ctx
        .input_dir
        .join(format!("{}/digest_template.html", cli.mode));
    tera.add_template_file(digest_template, Some("digest.html"))
        .unwrap();

    let render_template = ctx
        .input_dir
        .join(format!("{}/render_template.html", cli.mode));
    tera.add_template_file(render_template, Some("render.html"))
        .unwrap();

    println!("Loaded templates:");
    for template in tera.get_template_names() {
        println!("{template}");
    }

    match &cli.command {
        Some(Commands::Digest {}) => {
            println!("Creating digest.html");
            let get_posts = |action: ActionType| post_top.index(action);
            let blocks = vec![
                Block {
                    header: String::from("По комментариям"),
                    icon: icon_url("💬"),
                    cards: Card::create_cards(get_posts(ActionType::Replies), ActionType::Replies),
                    ..Block::default()
                },
                Block {
                    header: String::from("По реакциям"),
                    icon: icon_url("👏"),
                    cards: Card::create_cards(
                        get_posts(ActionType::Reactions),
                        ActionType::Reactions,
                    ),
                    ..Block::default()
                },
                Block {
                    header: String::from("По репостам"),
                    icon: icon_url("🔁"),
                    filter: String::from("filter-blue"),
                    cards: Card::create_cards(
                        get_posts(ActionType::Forwards),
                        ActionType::Forwards,
                    ),
                },
                Block {
                    header: String::from("По просмотрам"),
                    icon: icon_url("👁️"),
                    filter: String::from("filter-blue"),
                    cards: Card::create_cards(get_posts(ActionType::Views), ActionType::Views),
                },
            ]
            .into_iter()
            .filter(|b| b.cards.is_some())
            .collect::<Vec<Block>>();

            // Digest rendering

            let mut digest_context = tera::Context::new();
            digest_context.insert("blocks", &blocks);
            digest_context.insert("editor_choice_id", &cli.editor_choice_post_id);
            digest_context.insert("channel_name", &cli.channel_name.as_str());

            let rendered = tera.render("digest.html", &digest_context).unwrap();

            let digest_page_path = ctx.output_dir.join("digest.html");
            let mut file = fs::File::create(digest_page_path)?;
            file.write_all(rendered.as_bytes())?;
        }
        Some(Commands::Cards {
            replies,
            reactions,
            forwards,
            views,
        }) => {
            println!("Creating render.html and *.png cards");

            let card_post_index = [*replies - 1, *reactions - 1, *forwards - 1, *views - 1];

            let get_post =
                |action: ActionType| &post_top.index(action)[card_post_index[action as usize]];
            let cards = vec![
                Card {
                    header: String::from("Лучший по комментариям"),
                    icon: icon_url("💬"),
                    ..Card::create_card(get_post(ActionType::Replies), ActionType::Replies)
                },
                Card {
                    header: String::from("Лучший по реакциям"),
                    icon: icon_url("👏"),
                    ..Card::create_card(get_post(ActionType::Reactions), ActionType::Reactions)
                },
                Card {
                    header: String::from("Лучший по репостам"),
                    icon: icon_url("🔁"),
                    filter: String::from("filter-blue"),
                    ..Card::create_card(get_post(ActionType::Forwards), ActionType::Forwards)
                },
                Card {
                    header: String::from("Лучший по просмотрам"),
                    icon: icon_url("👁️"),
                    filter: String::from("filter-blue"),
                    ..Card::create_card(get_post(ActionType::Views), ActionType::Views)
                },
            ];
            let cards: Vec<Card> = cards.into_iter().filter(|c| c.count.is_some()).collect();

            // Card rendering

            let mut render_context = tera::Context::new();
            render_context.insert("cards", &cards);
            render_context.insert("editor_choice_id", &cli.editor_choice_post_id);
            render_context.insert("channel_name", &cli.channel_name.as_str());

            let rendered = tera.render("render.html", &render_context).unwrap();

            let render_page_path = ctx.output_dir.join("render.html");
            let mut file = fs::File::create(&render_page_path)?;
            file.write_all(rendered.as_bytes())?;

            // Browser part

            let viewport = chromiumoxide::handler::viewport::Viewport {
                width: 2000,
                height: 30000,
                device_scale_factor: Some(1.0),
                emulating_mobile: false,
                is_landscape: false,
                has_touch: false,
            };

            let (mut browser, mut handler) = Browser::launch(
                BrowserConfig::builder()
                    .window_size(2000, 30000)
                    .viewport(viewport)
                    .build()?,
            )
            .await?;

            // spawn a new task that continuously polls the handler
            let handle: tokio::task::JoinHandle<()> = tokio::task::spawn(async move {
                while let Some(h) = handler.next().await {
                    if h.is_err() {
                        break;
                    }
                }
            });

            // create a new browser page and navigate to the url
            let render_page = render_page_path.to_str().unwrap();
            let render_page_file = String::from("file://") + render_page;
            println!("Opening page for rendering: {render_page_file}");
            let page = browser.new_page(render_page_file).await?;

            // Wait to load? How much time is enough? Can we know the exact moment and wait synchronously?
            sleep(Duration::from_millis(100)).await;

            // find the search bar type into the search field and hit `Enter`,
            // this triggers a new navigation to the search result page
            let cards = page.find_elements("div").await?;

            // page.bring_to_front().await?;
            for (i, card) in cards.iter().enumerate() {
                let card_path = ctx.output_dir.join(format!("card_{:02}.png", i));
                let _ = card
                    .save_screenshot(CaptureScreenshotFormat::Png, &card_path)
                    .await?;
                println!("Generated: {}", card_path.to_str().unwrap());
            }

            browser.close().await?;
            let _ = handle.await;
        }
        _ => {}
    }

    // End

    Ok(())
}

fn main() -> Result<()> {
    runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async_main())
}
