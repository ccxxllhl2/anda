use agent_twitter_client::{models::Tweet, scraper::Scraper, search::SearchMode};
use anda_core::{
    ANONYMOUS, Agent, BoxError, CacheStoreFeatures, CompletionFeatures, RequestMeta, StateFeatures,
};
use anda_engine::{
    context::AgentCtx, engine::Engine, extension::character::CharacterAgent, rand_number,
};
use anda_kdb::KnowledgeStore;
use std::sync::Arc;
use tokio::{
    sync::RwLock,
    time::{Duration, sleep},
};
use tokio_util::sync::CancellationToken;

use crate::handler::ServiceStatus;

const MAX_HISTORY_TWEETS: i64 = 21;
const MAX_SEEN_TWEET_IDS: usize = 10000;
static IGNORE_COMMAND: &str = "IGNORE";

pub struct TwitterDaemon {
    engine: Arc<Engine>,
    agent: Arc<CharacterAgent<KnowledgeStore>>,
    scraper: Scraper,
    status: Arc<RwLock<ServiceStatus>>,
    min_interval_secs: u64,
}

impl TwitterDaemon {
    pub fn new(
        engine: Arc<Engine>,
        agent: Arc<CharacterAgent<KnowledgeStore>>,
        scraper: Scraper,
        status: Arc<RwLock<ServiceStatus>>,
        min_interval_secs: u64,
    ) -> Self {
        Self {
            engine,
            agent,
            scraper,
            status,
            min_interval_secs: min_interval_secs.max(60),
        }
    }

    pub async fn run(&self, cancel_token: CancellationToken) -> Result<(), BoxError> {
        {
            let ctx = self.engine.ctx_with(
                ANONYMOUS,
                &self.agent.as_ref().name(),
                RequestMeta::default(),
            )?;
            // load seen_tweet_ids from store
            ctx.cache_store_init("seen_tweet_ids", async { Ok(Vec::<String>::new()) })
                .await?;
            let (ids, _) = ctx.cache_store_get::<Vec<String>>("seen_tweet_ids").await?;
            log::info!("starting Twitter bot with {} seen tweets", ids.len());
        }

        let min_interval_secs = self.min_interval_secs;
        loop {
            {
                let status = self.status.read().await;
                if *status == ServiceStatus::Stopped {
                    log::info!("Twitter task stopped");
                    tokio::select! {
                        _ = cancel_token.cancelled() => {
                            return Ok(());
                        },
                        _ = sleep(Duration::from_secs(60)) => {},
                    }
                    continue;
                }
                log::info!("run a Twitter task");
                // release read lock
            }

            match self
                .scraper
                .search_tweets(
                    &format!("@{}", self.agent.character.handle.clone()),
                    20,
                    SearchMode::Latest,
                    None,
                )
                .await
            {
                Ok(mentions) => {
                    log::info!("fetch mentions: {} tweets", mentions.tweets.len());
                    for tweet in mentions.tweets {
                        if let Err(err) = self.handle_mention(tweet).await {
                            log::error!("handle mention error: {err:?}");
                        }

                        tokio::select! {
                            _ = cancel_token.cancelled() => {
                                return Ok(());
                            },
                            _ = sleep(Duration::from_secs(rand_number(3..=10))) => {},
                        }
                    }
                }
                Err(err) => {
                    log::error!("fetch mentions error: {err:?}");
                }
            }

            match rand_number(0..=10) {
                0 => {
                    if let Err(err) = self.handle_home_timeline().await {
                        log::error!("handle_home_timeline error: {err:?}");
                    }
                }
                n => {
                    log::info!("skip home timeline task by random {n}");
                }
            }

            match rand_number(0..=20) {
                0 => {
                    if let Err(err) = self.post_new_tweet().await {
                        log::error!("post_new_tweet error: {err:?}");
                    }
                }
                n => {
                    log::info!("skip post new tweet task by random {n}");
                }
            }

            // Sleep between tasks
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    return Ok(());
                },
                _ = sleep(Duration::from_secs(rand_number(min_interval_secs..=5 * min_interval_secs))) => {},
            }
        }
    }

    async fn post_new_tweet(&self) -> Result<(), BoxError> {
        let knowledges = self.agent.latest_knowledge(60 * 30, 3, None).await?;
        if knowledges.is_empty() {
            return Ok(());
        }

        log::info!("post new tweet with {} knowledges", knowledges.len());
        let ctx = self.engine.ctx_with(
            ANONYMOUS,
            &self.agent.as_ref().name(),
            RequestMeta {
                user: Some(self.engine.info().name.clone()),
                ..Default::default()
            },
        )?;
        let req = self
            .agent
            .character
            .to_request(
                "\
                Share a single brief thought or observation in one short sentence.\
                Be direct and concise. No questions, hashtags, or emojis.\
                "
                .to_string(),
                Some(self.engine.info().name.clone()),
            )
            .append_documents(knowledges.into());
        let res = ctx.completion(req, None).await?;
        match res.failed_reason {
            Some(reason) => Err(format!("Failed to generate response for tweet: {reason}").into()),
            None => {
                let _ = self.scraper.send_tweet(&res.content, None, None).await?;
                log::info!(
                    time_elapsed = ctx.time_elapsed().as_millis() as u64;
                    "post new tweet: {}",
                    res.content.chars().take(100).collect::<String>()
                );
                Ok(())
            }
        }
    }

    async fn handle_home_timeline(&self) -> Result<(), BoxError> {
        let ctx = self.engine.ctx_with(
            ANONYMOUS,
            &self.agent.as_ref().name(),
            RequestMeta {
                user: Some(self.engine.info().name.clone()),
                ..Default::default()
            },
        )?;

        let (mut seen_tweet_ids, _) = ctx.cache_store_get::<Vec<String>>("seen_tweet_ids").await?;
        if seen_tweet_ids.len() >= MAX_SEEN_TWEET_IDS {
            seen_tweet_ids.drain(0..MAX_SEEN_TWEET_IDS / 2);
        }
        let ids = if seen_tweet_ids.len() > 42 {
            seen_tweet_ids[(seen_tweet_ids.len() - 42)..].to_vec()
        } else {
            seen_tweet_ids.clone()
        };
        let tweets = self.scraper.get_home_timeline(1, ids).await?;
        log::info!("process home timeline, {} tweets", tweets.len());

        let mut likes = 0;
        let mut replys = 0;
        let mut quotes = 0;
        for tweet in tweets {
            let tweet_user = tweet["core"]["user_results"]["result"]["legacy"]["screen_name"]
                .as_str()
                .unwrap_or_else(|| tweet["legacy"]["user_id_str"].as_str().unwrap_or_default())
                .to_string();
            let tweet_content = tweet["legacy"]["full_text"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let tweet_id = tweet["legacy"]["id_str"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            if tweet_content.is_empty() || tweet_id.is_empty() {
                continue;
            }

            if tweet_user.to_lowercase() == self.agent.character.handle.to_lowercase() {
                // not replying to bot itself
                continue;
            }

            if seen_tweet_ids.contains(&tweet_id) {
                continue;
            }
            seen_tweet_ids.push(tweet_id.clone());

            let res: Result<(), BoxError> = async {
                if self.handle_like(&ctx, &tweet_content, &tweet_id).await? {
                    likes += 1;
                    if self.handle_quote(&ctx, &tweet_content, &tweet_id).await? {
                        // TODO: save tweet to knowledge store
                        quotes += 1;
                    } else {
                        self.handle_reply(&ctx, &tweet_content, &tweet_id).await?;
                        replys += 1;
                    }
                }
                Ok(())
            }
            .await;

            if let Err(err) = res {
                log::error!("handle home timeline {tweet_id} error: {err:?}");
            }

            sleep(Duration::from_secs(rand_number(3..=10))).await;
        }

        let _ = ctx
            .cache_store_set("seen_tweet_ids", seen_tweet_ids, None)
            .await;
        log::info!(
            "home timeline: likes {}, replys {}, quotes {}",
            likes,
            replys,
            quotes
        );
        Ok(())
    }

    async fn handle_mention(&self, tweet: Tweet) -> Result<(), BoxError> {
        let tweet_id = tweet.id.clone().unwrap_or_default();
        let tweet_text = tweet.text.clone().unwrap_or_default();
        let tweet_user = tweet.username.clone().unwrap_or_default();
        if tweet_text.is_empty() || tweet_user.is_empty() {
            return Ok(());
        }
        if tweet_user.to_lowercase() == self.agent.character.handle.to_lowercase() {
            // not replying to bot itself
            return Ok(());
        }
        let ctx = self.engine.ctx_with(
            ANONYMOUS,
            &self.agent.as_ref().name(),
            RequestMeta {
                user: Some(tweet_user.clone()),
                ..Default::default()
            },
        )?;
        let (mut seen_tweet_ids, _) = ctx.cache_store_get::<Vec<String>>("seen_tweet_ids").await?;

        if seen_tweet_ids.contains(&tweet_id) {
            return Ok(());
        }

        seen_tweet_ids.push(tweet_id.clone());

        let thread = self.build_conversation_thread(&tweet).await?;
        let messages: Vec<String> = thread
            .into_iter()
            .map(|t| {
                format!(
                    "{}: {:?}",
                    t.username.unwrap_or_default(),
                    t.text.unwrap_or_default()
                )
            })
            .collect();

        let tweet_text = if messages.len() <= 1 {
            tweet_text
        } else {
            messages.join("\n")
        };

        let res = self.agent.run(ctx.clone(), tweet_text, None).await?;
        if res.failed_reason.is_none() {
            // Reply to the original tweet
            let content = remove_quotes(res.content);
            let _ = self
                .scraper
                .send_tweet(&content, Some(&tweet_id), None)
                .await?;

            log::info!(
                tweet_user = tweet_user,
                tweet_id = tweet_id,
                chars = content.chars().count(),
                time_elapsed = ctx.time_elapsed().as_millis() as u64;
                "handle mention");
        }

        let _ = ctx
            .cache_store_set("seen_tweet_ids", seen_tweet_ids.clone(), None)
            .await;

        Ok(())
    }

    async fn build_conversation_thread(&self, tweet: &Tweet) -> Result<Vec<Tweet>, BoxError> {
        let mut thread = Vec::new();
        let mut current_tweet = Some(tweet.clone());
        let mut depth = 0;

        while let Some(tweet) = current_tweet {
            if tweet.text.is_some() {
                thread.push(tweet.clone());
            }

            if depth >= MAX_HISTORY_TWEETS {
                break;
            }

            sleep(Duration::from_secs(rand_number(1..=3))).await;
            current_tweet = match tweet.in_reply_to_status_id {
                Some(parent_id) => (self.scraper.get_tweet(&parent_id).await).ok(),
                None => None,
            };

            depth += 1;
        }

        thread.reverse();
        Ok(thread)
    }

    async fn handle_like(
        &self,
        ctx: &AgentCtx,
        tweet_content: &str,
        tweet_id: &str,
    ) -> Result<bool, BoxError> {
        if self.agent.should_like(ctx, tweet_content).await {
            let _ = self.scraper.like_tweet(tweet_id).await?;
            return Ok(true);
        }
        Ok(false)
    }

    async fn handle_reply(
        &self,
        ctx: &AgentCtx,
        tweet_content: &str,
        tweet_id: &str,
    ) -> Result<bool, BoxError> {
        let req = self
            .agent
            .character
            .to_request(
                format!("\
                    Respond the tweet AS **{}** would - only reply if your persona deems it necessary. When engaging:\n\
                    1. Use your character's unique voice and communication style naturally\n\
                    2. Keep responses to one authentic-sentence without hashtags\n\
                    3. Return {IGNORE_COMMAND} if your persona wouldn't respond to this tweet\n\n\
                    ## Tweet Content:\n{:?}\
                ", self.agent.character.name, tweet_content),
                Some(ctx.engine_name().to_owned()),
            );

        let res = ctx.completion(req, None).await?;
        match res.failed_reason {
            Some(reason) => Err(format!("Failed to generate response for tweet: {reason}").into()),
            None => {
                let content = remove_quotes(res.content);
                if content.is_empty() || content.contains(IGNORE_COMMAND) {
                    return Err("Ignore this tweet".into());
                }
                let _ = self
                    .scraper
                    .send_tweet(&content, Some(tweet_id), None)
                    .await?;
                Ok(true)
            }
        }
    }

    async fn handle_quote(
        &self,
        ctx: &AgentCtx,
        tweet_content: &str,
        tweet_id: &str,
    ) -> Result<bool, BoxError> {
        if self.agent.attention.should_quote(ctx, tweet_content).await {
            let req = self
            .agent
            .character
            .to_request(
                format!("\
                    Respond the tweet AS **{}** would - only reply if your persona deems it necessary. When engaging:\n\
                    1. Use your character's unique voice and communication style naturally\n\
                    2. Keep responses to one authentic-sentence without hashtags\n\
                    3. Return {IGNORE_COMMAND} if your persona wouldn't respond to this tweet\n\n\
                    ## Tweet Content:\n{:?}\
                ", self.agent.character.name, tweet_content),
                Some(ctx.engine_name().to_owned()),
            );

            let res = ctx.completion(req, None).await?;
            match res.failed_reason {
                Some(reason) => {
                    return Err(format!("Failed to generate response for tweet: {reason}").into());
                }
                None => {
                    let content = remove_quotes(res.content);
                    if content.is_empty() || content.contains(IGNORE_COMMAND) {
                        return Err("Ignore this tweet".into());
                    }
                    let _ = self
                        .scraper
                        .send_tweet(&content, Some(tweet_id), None)
                        .await?;
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
}

fn remove_quotes(s: String) -> String {
    let mut chars = s.chars();
    if chars.next() == Some('"') && chars.next_back() == Some('"') {
        chars.collect()
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    #[ignore]
    async fn test_x_api() {
        dotenv::dotenv().ok();

        let mut scraper = Scraper::new().await.unwrap();
        let cookie_string = std::env::var("TWITTER_COOKIES").expect("TWITTER_COOKIES is not set");

        scraper
            .set_from_cookie_string(&cookie_string)
            .await
            .unwrap();

        // scraper
        //     .login(
        //         std::env::var("TWITTER_USERNAME").unwrap(),
        //         std::env::var("TWITTER_PASSWORD").unwrap(),
        //         std::env::var("TWITTER_EMAIL").ok(),
        //         std::env::var("TWITTER_2FA_SECRET").ok(),
        //     )
        //     .await
        //     .unwrap();

        {
            let res = scraper
                .search_tweets(&format!("@{}", "ICPandaDAO"), 5, SearchMode::Latest, None)
                .await
                .unwrap();
            for tweet in res.tweets {
                // let data = serde_json::to_string_pretty(&tweet).unwrap();
                // println!("{}", data);
                let tweet_user = tweet.username.unwrap_or_default();
                let tweet_content = tweet.text.unwrap_or_default();
                let tweet_id = tweet.id.unwrap_or_default();
                println!("\n\n{}: {} - {}", tweet_user, tweet_id, tweet_content);

                println!("----------------------");
            }
        }

        {
            let tweets = scraper.get_home_timeline(1, Vec::new()).await.unwrap();
            for tweet in &tweets {
                let tweet_user = tweet["core"]["user_results"]["result"]["legacy"]["screen_name"]
                    .as_str()
                    .unwrap_or_else(|| tweet["legacy"]["user_id_str"].as_str().unwrap_or_default())
                    .to_string();
                let tweet_content = tweet["legacy"]["full_text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let tweet_id = tweet["legacy"]["id_str"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                println!("{}: {} - {}", tweet_user, tweet_id, tweet_content);
            }
        }
        // let tweets = serde_json::to_string_pretty(&tweets).unwrap();
        // std::fs::write("home_timeline_tweets.json", tweets).unwrap();
    }
}
