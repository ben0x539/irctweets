#![feature(type_ascription)]

use {
    std::{
        path::{Path, PathBuf},
        fs,
        io,
    },
    irc::client::prelude::*,
    anyhow::{Result, anyhow},
    tokio::{runtime::Runtime, stream::StreamExt},
    tracing::{trace, debug, info, error, span, Level},
};

#[derive(Debug, structopt::StructOpt)]
struct Args {
    #[structopt(short, long, default_value = "irctweets.toml")]
    config: PathBuf,
}

struct App {
    db: rusqlite::Connection,
    link_finder: linkify::LinkFinder,
}

#[derive(Debug, PartialEq, serde_derive::Deserialize)]
struct Config {
    db: PathBuf,
    irc: irc::client::data::config::Config,
}

impl Config {
    fn load<P: AsRef<Path>>(path: &P) -> Result<Config> {
        let file_contents = fs::read_to_string(&path)?;
        let config = toml::from_str(&file_contents)?;
        Ok(config)
    }
}

impl App {
    fn init_db(&self) -> Result<()> {
        self.db.execute("
            create table if not exists tweet (
                id integer primary key,
                tweet_id integer unique not null,
                retweet_id integer,
                error varchar
            )
        ", rusqlite::NO_PARAMS)?;

        self.db.execute("
            create table if not exists occurence (
                id integer primary key,
                tweet integer not null,
                line integer not null,
                unique(tweet, line)
            )
        ", rusqlite::NO_PARAMS)?;

        self.db.execute("
            create table if not exists line (
                id integer primary key,
                timestamp integer not null,
                prefix text not null,
                target text not null,
                msg text not null
            )
        ", rusqlite::NO_PARAMS)?;

        Ok(())
    }

    fn handle_message(&self, _client: &Client, message: &Message)
            -> Result<()> {
        let (prefix, target, msg) = match message {
            Message {
                prefix: Some(prefix),
                command: Command::PRIVMSG(target, msg),
            .. } =>  (prefix, target, msg),
            _ => return Ok(()),
        };

        trace!(%target, %msg, "privmsg");
        let mut maybe_line = None;

        for link in self.link_finder.links(&msg) {
            let link = link.as_str();
            let span = span!(Level::TRACE, "link", %link);
            let _enter = span.enter();
            trace!("link");

            let tweet_id = match get_tweet(link) {
                Some(tweet_id) => tweet_id,
                None => continue,
            };
            info!(%tweet_id, "found tweet id");

            let line = match maybe_line {
                Some(line) => line,
                None => {
                    let line =
                        self.insert_line(&prefix.to_string(), &target, &msg)?;
                    maybe_line = Some(line);
                    line
                }
            };

            let tweet = self.maybe_insert_tweet(tweet_id)?;
            self.maybe_insert_occurence(line, tweet)?;
        }

        Ok(())
    }

    fn insert_line(&self, prefix: &str, target: &str, msg: &str)
            -> Result<i64> {
        self.db.execute("
            insert into line(timestamp, prefix, target, msg)
            values(datetime(), ?, ?, ?);
        ", &[prefix, target, msg])?;

        Ok(self.db.last_insert_rowid())
    }

    fn maybe_insert_tweet(&self, tweet_id: u64) -> Result<i64> {
        self.db.execute("
            insert or ignore into tweet(tweet_id)
            values(?);
        ", &[tweet_id as i64])?;

        let tweet = self.db.query_row("
            select id from tweet where tweet_id = ?
        ", &[tweet_id as i64], |row| row.get(0))?;

        Ok(tweet)
    }

    fn maybe_insert_occurence(&self, line: i64, tweet: i64) -> Result<()> {
        self.db.execute("
            insert or ignore into occurence(tweet, line)
            values(?, ?);
        ", &[tweet, line])?;

        Ok(())
    }
}

fn get_tweet(url_str: &str) -> Option<u64> {
    let url = url::Url::parse(url_str).ok()?;

    if url.scheme() != "https" { return None; }

    const TWITTER_HOSTS: &[&str] = &[
        "twitter.com",
        "www.twitter.com",
        "mobile.twitter.com",
        "m.twitter.com",
    ];

    let host = url.host_str()?;
    if TWITTER_HOSTS.iter().all(|&h| h != host) { return None; }

    let mut path_segments = url.path_segments()?;
    path_segments.next()?;
    if path_segments.next() != Some("status") { return None; }
    let tweet_id = path_segments.next()?.parse().ok()?;

    Some(tweet_id)
}

fn r<T>(r: irc::error::Result<T>) -> Result<T> {
    match r {
        Ok(v) => Ok(v),
        Err(e) => Err(anyhow!(e)),
    }
}

#[paw::main]
fn main(args: Args) -> Result<()> {
    let subscriber = tracing_subscriber::fmt::Subscriber::builder()
        .with_max_level(Level::DEBUG)
        .compact()
        .with_writer(io::stderr)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;
    debug!(?args, "starting up");

    let config = Config::load(&args.config)?;

    let db = rusqlite::Connection::open(&config.db)?;

    let mut link_finder = linkify::LinkFinder::new();
    link_finder.kinds(&[linkify::LinkKind::Url]);

    let app = App { db, link_finder };

    app.init_db()?;

    Runtime::new()?.block_on(async move {
        let mut client = r(Client::from_config(config.irc.clone()).await)?;
        r(client.identify())?;
        let mut stream = r(client.stream())?;
        while let Some(message) = r(stream.next().await.transpose())? {
            let span = span!(Level::TRACE, "message", %message);
            let _enter = span.enter();
            if let Err(e) = app.handle_message(&client, &message) {
                error!(%e, %message, "couldn't handle message");
            }
        }

        Ok(()): Result<()>
    })?;


    Ok(())
}
