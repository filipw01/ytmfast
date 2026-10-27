//! Diagnostic: whether the playback session's token can read the friend
//! activity feed, run with the stored playback credential.
//!
//!   cargo run --example friends_probe

use librespot_core::{Session, SessionConfig, cache::Cache};

fn main() -> anyhow::Result<()> {
    fastframe_log::Logging::new("ytmfast", env!("CARGO_PKG_VERSION"))
        .filter("warn")
        .init()?;
    let dirs = ytmfast::paths::AppDirs::discover().dirs;
    let cache = Cache::new::<&std::path::Path>(None, None, None, None)?.with_memory_credentials();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let store = ytmfast::credentials::Store::new(dirs);
        let loaded = store
            .lease(ytmfast::credentials::Slot::Playback)
            .load()
            .await?;
        if let Some(warning) = loaded.warning {
            eprintln!("{warning}");
        }
        let Some(ytmfast::credentials::Grant::Playback(credentials)) = loaded.grant else {
            anyhow::bail!("Enable playback in Ytmfast first");
        };
        let session = Session::new(SessionConfig::default(), Some(cache));
        session.connect(credentials, false).await?;
        println!("connected as {}", session.username());

        let names = tokio::sync::Mutex::default();
        match ytmfast::session_reads::friend_activity(&session, &names).await {
            Ok(friends) => {
                println!("friends -> ok: {} friends", friends.len());
                for friend in friends {
                    println!(
                        "  {} | {} - {} | {:?} | playing {}",
                        friend.user_name,
                        friend.artist_name,
                        friend.track_name,
                        friend.context_name,
                        friend.playing
                    );
                }
            }
            Err(error) => println!("friends -> ERR: {error}"),
        }
        anyhow::Ok(())
    })?;
    Ok(())
}
