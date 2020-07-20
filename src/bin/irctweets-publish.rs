#![feature(type_ascription)]

use {
    std::{
        path::{Path, PathBuf},
        fs,
        io,
        time,
    },
    anyhow::Result,
    rusqlite::types::ToSql,
    tokio::time::delay_for,
    tracing::{span, trace, error, info, Level},
};

#[derive(Debug, structopt::StructOpt)]
struct Args {
    #[structopt(short, long, default_value = "irctweets.toml")]
    config: PathBuf,
}

struct App {
    db: rusqlite::Connection,
    creds: egg_mode::Token,
}

#[derive(Debug, PartialEq, serde_derive::Deserialize)]
struct Config {
    db: PathBuf,
    twitter: TwitterConfig,
}

impl Config {
    fn load<P: AsRef<Path>>(path: &P) -> Result<Config> {
        let file_contents = fs::read_to_string(&path)?;
        let config = toml::from_str(&file_contents)?;
        Ok(config)
    }
}

#[derive(Debug, PartialEq, serde_derive::Deserialize)]
struct TwitterConfig {
    consumer_token: String,
    consumer_token_secret: String,
    access_token: String,
    access_token_secret: String,
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

        Ok(())
    }

    fn get_new_tweets(&self, limit: i32) -> Result<Vec<u64>> {
        let mut stmt = self.db.prepare("
            select tweet_id
            from tweet
            where retweet_id is null and error is null
            limit ?;
        ")?;

        let mut rows = stmt.query(&[&limit])?;
        let mut ids = Vec::new();
        while let Some(row) = rows.next()? {
            let id: i64 = row.get(0)?;
            ids.push(id as u64);
        }

        Ok(ids)
    }

    fn store_retweet_id(&self, tweet_id: u64, retweet_id: u64)
            -> Result<()> {
        let tweet_id = tweet_id as i64;
        let retweet_id = retweet_id as i64;
        self.db.execute("
            update tweet
            set retweet_id = ?
            where tweet_id is ? and retweet_id is null and error is null
        ", &[retweet_id, tweet_id])?;

        Ok(())
    }

    fn store_error(&self, tweet_id: u64, error: String)
            -> Result<()> {
        let tweet_id = tweet_id as i64;
        self.db.execute("
            update tweet
            set error = ?
            where tweet_id is ? and retweet_id is null and error is null
        ", &[&error, &tweet_id]: &[&dyn ToSql; 2])?;

        Ok(())
    }

    async fn tick(&self) -> Result<()> {
        let tweet_ids = self.get_new_tweets(100)?;
        if tweet_ids.len() == 0 {
            return Ok(());
        }

        for tweet_id in tweet_ids {
            let span = span!(Level::INFO, "processing tweet", %tweet_id);
            let _enter = span.enter();
            let result =
                egg_mode::tweet::retweet(tweet_id, &self.creds).await;
            match result {
                Ok(r) => {
                    let retweet = r.response;
                    info!(%retweet.id, "retweeted");
                    self.store_retweet_id(tweet_id, retweet.id)?;
                }, Err(e) => {
                    error!(%e, "couldn't retweet");
                    self.store_error(tweet_id, e.to_string())?;
                },
            }
        }

        Ok(())
    }

    async fn run(&self) -> Result<()> {
        loop {
            if let Err(e) = self.tick().await {
                error!(%e, "error during tick");
            }

            delay_for(time::Duration::from_secs(5)).await;
        }
    }
}

#[paw::main]
#[tokio::main]
async fn main(args: Args) -> Result<()> {
    let subscriber = tracing_subscriber::fmt::Subscriber::builder()
        .with_max_level(Level::TRACE)
        .compact()
        .with_writer(io::stderr)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;
    trace!(?args, "starting up");

    let config = Config::load(&args.config)?;

    let creds = egg_mode::Token::Access {
        consumer: egg_mode::KeyPair::new(config.twitter.consumer_token,
            config.twitter.consumer_token_secret),
        access: egg_mode::KeyPair::new(config.twitter.access_token,
            config.twitter.access_token_secret),
    };

    let db = rusqlite::Connection::open(&config.db)?;

    let app = App { db, creds };

    app.init_db()?;

    app.run().await?;

    Ok(())
}
