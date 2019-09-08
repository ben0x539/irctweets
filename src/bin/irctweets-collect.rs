use {
    std::{
        path::PathBuf,
        io,
    },
    irc::client::prelude::*,
    failure::Error,
    tracing::{trace, error, span, Level},
};

#[derive(Debug, structopt::StructOpt)]
struct Args {
    #[structopt(short, long, default_value = "irctweets.toml")]
    config: PathBuf,
    #[structopt(short, long, default_value = "irctweets.sqlite")]
    db: PathBuf,
}

struct App {
    db: rusqlite::Connection,
    link_finder: linkify::LinkFinder,
}

impl App {
    fn init_db(&self) -> Result<(), Error> {
        self.db.execute("
            create table if not exists tweet (
                id integer primary key,
                tweet_id integer unique not null,
                retweet_id integer
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

    fn handle_message(&self, _client: &IrcClient, message: &Message)
            -> Result<(), Error> {
        if let Message { command: Command::PRIVMSG(target, msg), ..} = message {
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
                trace!(%tweet_id);

                let line = match maybe_line {
                    Some(line) => line,
                    None => {
                        let prefix = message.prefix.as_ref()
                            .map(|s| &s[..]).unwrap_or("");
                        let line =
                            self.insert_line(prefix, &target, &msg)?;
                        maybe_line = Some(line);
                        line
                    }
                };

                let tweet = self.maybe_insert_tweet(tweet_id)?;
                self.maybe_insert_occurence(line, tweet)?;
            }
        }

        Ok(())
    }

    fn insert_line(&self, prefix: &str, target: &str, msg: &str)
            -> Result<i64, Error> {
        self.db.execute("
            insert into line(timestamp, prefix, target, msg)
            values(datetime(), ?, ?, ?);
        ", &[prefix, target, msg])?;

        Ok(self.db.last_insert_rowid())
    }

    fn maybe_insert_tweet(&self, tweet_id: i64) -> Result<i64, Error> {
        self.db.execute("
            insert or ignore into tweet(tweet_id)
            values(?);
        ", &[tweet_id])?;

        let tweet = self.db.query_row("
            select id from tweet where tweet_id = ?
        ", &[tweet_id], |row| row.get(0))?;

        Ok(tweet)
    }

    fn maybe_insert_occurence(&self, tweet: i64, line: i64)
            -> Result<(), Error> {
        self.db.execute("
            insert or ignore into occurence(tweet, line)
            values(?, ?);
        ", &[tweet, line])?;

        Ok(())
    }
}

fn get_tweet(url_str: &str) -> Option<i64> {
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

#[paw::main]
fn main(args: Args) -> Result<(), Error> {
    let subscriber = tracing_subscriber::fmt::Subscriber::builder()
        //.with_filter("attrs_basic=trace")
        .compact()
        .with_writer(io::stderr)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    let config = Config::load(&args.config)?;

    let db = rusqlite::Connection::open(&args.db)?;

    let mut link_finder = linkify::LinkFinder::new();
    link_finder.kinds(&[linkify::LinkKind::Url]);

    let app = App { db, link_finder };

    app.init_db()?;

    let mut reactor = IrcReactor::new()?;
    let client = reactor.prepare_client_and_connect(&config)?;
    client.identify()?;
    reactor.register_client_with_handler(client, move |client, message| {
        let span = span!(Level::TRACE, "message", message = %message);
        let _enter = span.enter();
        if let Err(e) = app.handle_message(client, &message) {
            error!(%e, "couldn't handle message");
        }

        Ok(())
    });

    reactor.run()?;

    Ok(())
}
