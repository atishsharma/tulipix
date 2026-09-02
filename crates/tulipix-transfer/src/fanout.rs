//! One send, several destinations.
//!
//! Concurrency stops at the destination boundary, deliberately. `web/app.js`
//! sends a queue sequentially because parallel writes to one disk are slower
//! and one active write keeps progress honest — that reasoning still holds, so
//! each lane keeps its own sequential loop. What is parallel here is the lanes
//! themselves: separate sockets to separate machines, where the link is the
//! bottleneck and one disk is not.
//!
//! A lane also waits: an upload is refused with 403 until a person at the far
//! end accepts the offer, so a lane calls [`crate::peer::Peer::await_answer`]
//! before it pushes a single byte. A declined or unanswered lane becomes
//! `LaneState::Failed("declined")` — a reason a person can act on, not a
//! transport error indistinguishable from a dropped connection.

use std::path::PathBuf;
use std::time::Duration;

use sqlx::SqlitePool;

use crate::server::now_secs;

/// Somewhere a job is going.
#[derive(Clone, Debug)]
pub struct Destination {
    pub name: String,
    pub base: String,
    pub token: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaneState {
    Waiting,
    Sending,
    Done,
    Failed(String),
}

/// One destination's share of a job.
#[derive(Clone, Debug)]
pub struct Lane {
    pub name: String,
    pub sent: u64,
    pub total: u64,
    pub state: LaneState,
}

/// Send every file to every destination, one lane per destination.
///
/// A lane that fails is recorded and left behind; it never cancels a sibling,
/// and its `sent` count is the offset a retry resumes from.
pub async fn send(
    files: Vec<PathBuf>,
    to: Vec<Destination>,
    pool: Option<SqlitePool>,
) -> Vec<Lane> {
    let job = match pool.as_ref() {
        Some(p) if to.len() > 1 => crate::ledger::next_job_id(p).await.ok(),
        _ => None,
    };

    // (path, name, bytes) for every file that can actually be sent — built
    // once, borrowed read-only by every lane below.
    let manifest: Vec<(PathBuf, String, u64)> = files
        .iter()
        .filter_map(|p| {
            let name = p.file_name()?.to_str()?.to_string();
            let bytes = std::fs::metadata(p).ok()?.len();
            Some((p.clone(), name, bytes))
        })
        .collect();
    let total: u64 = manifest.iter().map(|(_, _, b)| *b).sum();
    let offer_files: Vec<(String, u64)> =
        manifest.iter().map(|(_, name, b)| (name.clone(), *b)).collect();

    let tasks = to.into_iter().map(|dest| {
        let manifest = &manifest;
        let offer_files = &offer_files;
        let pool = pool.as_ref();
        async move {
            let mut lane = Lane {
                name: dest.name.clone(),
                sent: 0,
                total,
                state: LaneState::Sending,
            };

            let outcome = async {
                let peer = crate::peer::connect(&dest.base, &dest.token)?;
                let offer = peer.offer(offer_files).await?;
                // A push is refused until a person at the far end accepts, so
                // the lane waits here rather than failing on first contact.
                // Two minutes is somebody noticing a dialog; past that the
                // lane gives up rather than holding a socket open all day.
                if !peer.await_answer(offer, Duration::from_secs(120)).await? {
                    return Err(anyhow::anyhow!("declined"));
                }
                for (path, _, _) in manifest.iter() {
                    // Sequential within the lane — see the module comment.
                    lane.sent += peer.push(offer, path, 0).await?;
                }
                Ok::<(), anyhow::Error>(())
            }
            .await;

            lane.state = match outcome {
                Ok(()) => LaneState::Done,
                Err(e) => LaneState::Failed(e.to_string()),
            };

            if let Some(p) = pool {
                for (path, name, bytes) in manifest.iter() {
                    // The real source path, not empty: this is what a
                    // "reveal" click in Recent Transfers opens, and an empty
                    // path is the exact bug `Row::sent`'s own doc comment
                    // describes ("a sent row without one was the only kind
                    // that could not be opened").
                    let where_from = path.to_string_lossy().into_owned();
                    let mut row =
                        crate::ledger::Row::sent(name, &where_from, *bytes as i64, &dest.name);
                    if let Some(j) = job {
                        row = row.for_job(j);
                    }
                    if matches!(lane.state, LaneState::Failed(_)) {
                        row = row.failed();
                    }
                    let _ = crate::ledger::record(p, row, now_secs() as i64).await;
                }
            }

            lane
        }
    });

    futures_util::future::join_all(tasks).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    async fn a_receiver() -> (crate::server::Running, String, tempfile::TempDir) {
        let inbox = tempfile::tempdir().unwrap();
        let run = crate::server::start(crate::server::Config {
            inbox: inbox.path().to_path_buf(),
            bind: "127.0.0.1:0".into(),
            pool: None,
            tls: false,
        })
        .await
        .unwrap();
        let base = format!("http://127.0.0.1:{}", run.port);
        (run, base, inbox)
    }

    async fn paired(run: &crate::server::Running, base: &str) -> String {
        let (_c, theirs) = crate::peer::pair(base, "sender:fp").await.unwrap();
        assert!(run.approve_pairing(), "the person at the far machine says the digits match");
        crate::peer::confirm(base, &theirs).await.unwrap()
    }

    #[tokio::test]
    async fn a_dead_lane_does_not_cancel_its_siblings() {
        let src = tempfile::tempdir().unwrap();
        let file = src.path().join("fan.bin");
        let mut f = std::fs::File::create(&file).unwrap();
        f.write_all(b"0123456789").unwrap();
        drop(f);

        let (run_a, base_a, inbox_a) = a_receiver().await;
        let (run_b, base_b, inbox_b) = a_receiver().await;
        let (run_c, base_c, _inbox_c) = a_receiver().await;

        let token_a = paired(&run_a, &base_a).await;
        let token_b = paired(&run_b, &base_b).await;
        let token_c = paired(&run_c, &base_c).await;

        let dests = vec![
            Destination { name: "a".into(), base: base_a.clone(), token: token_a },
            Destination { name: "b".into(), base: base_b.clone(), token: token_b },
            Destination { name: "c".into(), base: base_c.clone(), token: token_c },
        ];

        // c refuses everything: it is the lane that dies. It never even gets
        // to the point of announcing an offer, since the socket is gone.
        run_c.stop().await;

        // The offers for a and b are not accepted until a beat after `send`
        // is already running — otherwise this would only prove the accept
        // path exists, not that a lane actually waits on it. Reading the
        // pending offer back out is the only way in: there is no route that
        // lists ids from outside, so this goes straight at each receiver's
        // own state.
        let acceptor = async {
            tokio::time::sleep(Duration::from_millis(300)).await;
            for run in [&run_a, &run_b] {
                let pending = run.state.pending_offers();
                let id = pending.first().expect("the offer is announced by now").0;
                assert!(run.accept_offer(id), "accept the queued offer, a beat late");
            }
        };

        let (lanes, ()) = tokio::join!(send(vec![file.clone()], dests, None), acceptor);
        assert_eq!(lanes.len(), 3, "one lane per destination, always");

        let done: Vec<&Lane> =
            lanes.iter().filter(|l| matches!(l.state, LaneState::Done)).collect();
        assert_eq!(done.len(), 2, "the two live destinations completed");
        assert!(
            lanes.iter().any(|l| matches!(l.state, LaneState::Failed(_))),
            "the dead one is reported, not swallowed"
        );

        assert_eq!(std::fs::read(inbox_a.path().join("fan.bin")).unwrap(), b"0123456789");
        assert_eq!(std::fs::read(inbox_b.path().join("fan.bin")).unwrap(), b"0123456789");

        run_a.stop().await;
        run_b.stop().await;
    }
}
