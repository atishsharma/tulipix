//! `np.p4.music.similar` — three questions answered from the library itself.
//!
//! *What next*, when the queue runs dry. *Who else sounds like this*, for an
//! artist page. *Is this the same recording twice*, for a library that grew
//! over fifteen years. All three are the same operation underneath: compare
//! tracks on what the scan pass already measured, and rank.
//!
//! Nothing here calls out. The comparison is the `dsp-v1` fingerprint from
//! [`crate::analysis`] plus the tags already in `track_meta`, so every answer is
//! something the user owns and can play immediately — which is the point of
//! doing it locally rather than asking a recommendation API what it thinks.
//!
//! The fingerprint is optional throughout. A library that has never run the
//! analysis pass still gets artist, genre, era and tag-tempo matching; it just
//! loses the strongest signal. Duplicate detection is the exception and returns
//! nothing without it, because tags are exactly what cannot be trusted there.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use sqlx::SqlitePool;

use crate::{analysis, bpm_key, embeddings};

// ------------------------------------------------------------------ weights --

/// How hard each agreement pulls a candidate up the ranking.
///
/// Sound outweighs any single tag because it is the only signal that cannot be
/// wrong: a genre is one word somebody typed, and half a library has it blank.
const W_SOUND: f32 = 4.0;
const W_ARTIST: f32 = 2.0;
const W_GENRE: f32 = 1.5;
const W_ERA: f32 = 0.75;
const W_TEMPO: f32 = 1.0;
const W_KEY: f32 = 1.0;

/// Years apart that still count as the same era.
const ERA_YEARS: i64 = 5;
/// Fractional tempo difference that still counts as the same tempo. 8% is about
/// where a listener stops hearing a change of gear.
const TEMPO_TOL: f64 = 0.08;
/// At most this many tracks by one artist in a generated run. Without a cap the
/// most-similar list to any track is the rest of its own album, which is not a
/// station — it is the album you were already playing.
const MAX_PER_ARTIST: usize = 2;

// -------------------------------------------------------------------- facts --

/// What one track can be compared on, tags only.
#[derive(Debug, Clone, sqlx::FromRow)]
struct Facts {
    item_id: i64,
    artist_id: Option<i64>,
    genre: Option<String>,
    year: Option<i64>,
    bpm: Option<f64>,
    music_key: Option<String>,
    duration_s: Option<f64>,
}

/// Every playable music track's comparable facts.
///
/// One query for the whole library rather than one per candidate: at a few
/// dozen bytes a row even a 100k-track library is a handful of megabytes, and
/// the alternative is a query per comparison.
const FACTS_SQL: &str = "SELECT tm.item_id AS item_id, tm.artist_id AS artist_id, \
     tm.genre AS genre, tm.year AS year, tm.bpm AS bpm, tm.music_key AS music_key, \
     tm.duration_s AS duration_s \
     FROM track_meta tm JOIN items i ON i.id = tm.item_id \
     WHERE i.section = 'music' AND i.missing_since IS NULL \
       AND COALESCE(tm.is_audiobook, 0) = 0";

async fn facts(pool: &SqlitePool) -> Result<Vec<Facts>> {
    Ok(sqlx::query_as::<_, Facts>(FACTS_SQL).fetch_all(pool).await?)
}

/// Every stored `dsp-v1` fingerprint, by item.
async fn prints(pool: &SqlitePool) -> Result<HashMap<i64, Vec<f32>>> {
    let rows: Vec<(i64, Vec<u8>)> =
        sqlx::query_as("SELECT item_id, vec FROM track_embeddings WHERE model = ?")
            .bind(analysis::FINGERPRINT_MODEL)
            .fetch_all(pool)
            .await?;
    Ok(rows
        .into_iter()
        .map(|(id, b)| (id, embeddings::from_bytes(&b)))
        .collect())
}

/// Tag agreement between two tracks, 0.0 upward. Sound is scored separately —
/// this is only what the metadata says.
fn tag_score(a: &Facts, b: &Facts) -> f32 {
    let mut score = 0.0;
    if let (Some(x), Some(y)) = (a.artist_id, b.artist_id) {
        if x == y {
            score += W_ARTIST;
        }
    }
    if let (Some(x), Some(y)) = (a.genre.as_deref(), b.genre.as_deref()) {
        // Case-insensitive: "Trip-Hop" and "trip-hop" are one genre, and the
        // tag came from whatever wrote the file.
        if !x.is_empty() && x.eq_ignore_ascii_case(y) {
            score += W_GENRE;
        }
    }
    if let (Some(x), Some(y)) = (a.year, b.year) {
        if x > 0 && y > 0 && (x - y).abs() <= ERA_YEARS {
            score += W_ERA;
        }
    }
    if let (Some(x), Some(y)) = (a.bpm, b.bpm) {
        if x > 0.0 && y > 0.0 && ((x - y).abs() / x.max(y)) <= TEMPO_TOL {
            score += W_TEMPO;
        }
    }
    if let (Some(x), Some(y)) = (a.music_key.as_deref(), b.music_key.as_deref()) {
        if bpm_key::harmonic(x, y) {
            score += W_KEY;
        }
    }
    score
}

// ------------------------------------------------------------------ station --

/// Tracks to keep playing with, once the queue has run out.
///
/// Seeded from the last few things played rather than from one, so a run does
/// not swing on whichever track happened to be last. Every candidate is scored
/// against every seed and the scores averaged: something that matches all three
/// seeds beats something that matches one of them perfectly.
///
/// `exclude` is anything already spoken for — the queue, usually — so a station
/// never offers a track that is about to play anyway.
pub async fn station(
    pool: &SqlitePool,
    seeds: &[i64],
    exclude: &[i64],
    limit: usize,
) -> Result<Vec<i64>> {
    if seeds.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }
    let all = facts(pool).await?;
    let prints = prints(pool).await?;

    let by_id: HashMap<i64, &Facts> = all.iter().map(|f| (f.item_id, f)).collect();
    let seed_facts: Vec<&Facts> = seeds.iter().filter_map(|id| by_id.get(id).copied()).collect();
    if seed_facts.is_empty() {
        return Ok(Vec::new());
    }

    let skip: HashSet<i64> = seeds.iter().chain(exclude).copied().collect();

    let mut scored: Vec<(f32, i64, Option<i64>)> = all
        .iter()
        .filter(|c| !skip.contains(&c.item_id))
        .filter_map(|c| {
            let total: f32 = seed_facts
                .iter()
                .map(|s| {
                    let sound = match (prints.get(&s.item_id), prints.get(&c.item_id)) {
                        (Some(a), Some(b)) => {
                            // Cosine runs -1..1 and a negative one means "the
                            // opposite shape", which is not a reason to rank a
                            // track below one we know nothing about.
                            embeddings::cosine(a, b).max(0.0) * W_SOUND
                        }
                        _ => 0.0,
                    };
                    sound + tag_score(s, c)
                })
                .sum();
            (total > 0.0).then_some((total / seed_facts.len() as f32, c.item_id, c.artist_id))
        })
        .collect();

    // total_cmp, not partial_cmp().unwrap(): a stored fingerprint is raw bytes
    // from the DB and every bit pattern decodes to a valid f32, so a corrupt row
    // can put a NaN in here. This crate builds with panic = "abort".
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));

    let mut per_artist: HashMap<i64, usize> = HashMap::new();
    let mut out = Vec::with_capacity(limit);
    for (_, id, artist) in scored {
        if let Some(a) = artist {
            let n = per_artist.entry(a).or_insert(0);
            if *n >= MAX_PER_ARTIST {
                continue;
            }
            *n += 1;
        }
        out.push(id);
        if out.len() == limit {
            break;
        }
    }
    Ok(out)
}

// ---------------------------------------------------------- similar artists --

/// One artist reduced to what they can be compared on.
struct ArtistShape {
    genres: HashSet<String>,
    year: Option<f64>,
    centroid: Option<Vec<f32>>,
}

/// Artists on these shelves who sound like `artist_id`, best first, with the
/// score that put them there.
///
/// An artist is the average of their tracks: the genres they are tagged with,
/// the middle of their years, and the centroid of their fingerprints. That last
/// one is why this works on artists whose genre tag is blank — a centroid of
/// unit vectors still points somewhere.
pub async fn similar_artists(pool: &SqlitePool, artist_id: i64, k: usize) -> Result<Vec<(i64, f32)>> {
    if k == 0 {
        return Ok(Vec::new());
    }
    let all = facts(pool).await?;
    let prints = prints(pool).await?;

    let mut shapes: HashMap<i64, ArtistShape> = HashMap::new();
    let mut years: HashMap<i64, Vec<i64>> = HashMap::new();
    let mut sums: HashMap<i64, (Vec<f32>, usize)> = HashMap::new();
    for f in &all {
        let Some(aid) = f.artist_id else { continue };
        let shape = shapes.entry(aid).or_insert_with(|| ArtistShape {
            genres: HashSet::new(),
            year: None,
            centroid: None,
        });
        if let Some(g) = f.genre.as_deref() {
            if !g.trim().is_empty() {
                shape.genres.insert(g.trim().to_ascii_lowercase());
            }
        }
        if let Some(y) = f.year {
            if y > 0 {
                years.entry(aid).or_default().push(y);
            }
        }
        if let Some(v) = prints.get(&f.item_id) {
            let slot = sums.entry(aid).or_insert_with(|| (vec![0.0; v.len()], 0));
            if slot.0.len() == v.len() {
                for (acc, x) in slot.0.iter_mut().zip(v) {
                    *acc += x;
                }
                slot.1 += 1;
            }
        }
    }
    for (aid, ys) in years {
        if let Some(shape) = shapes.get_mut(&aid) {
            shape.year = Some(ys.iter().sum::<i64>() as f64 / ys.len() as f64);
        }
    }
    for (aid, (sum, n)) in sums {
        if n == 0 {
            continue;
        }
        // Re-normalise: a mean of unit vectors is not one, and cosine against an
        // un-normalised centroid would rank prolific artists higher for it.
        let norm = sum.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > f32::EPSILON {
            if let Some(shape) = shapes.get_mut(&aid) {
                shape.centroid = Some(sum.into_iter().map(|v| v / norm).collect());
            }
        }
    }

    let Some(seed) = shapes.get(&artist_id) else { return Ok(Vec::new()) };
    let mut scored: Vec<(i64, f32)> = shapes
        .iter()
        .filter(|(id, _)| **id != artist_id)
        .filter_map(|(id, other)| {
            let mut score = 0.0f32;
            // Jaccard over genres: an artist tagged with five genres that shares
            // one with you is a weaker match than one tagged with exactly yours.
            let shared = seed.genres.intersection(&other.genres).count();
            if shared > 0 {
                let union = seed.genres.union(&other.genres).count().max(1);
                score += W_GENRE * (shared as f32 / union as f32);
            }
            if let (Some(a), Some(b)) = (seed.year, other.year) {
                let gap = (a - b).abs();
                if gap <= ERA_YEARS as f64 {
                    score += W_ERA * (1.0 - gap as f32 / ERA_YEARS as f32);
                }
            }
            if let (Some(a), Some(b)) = (&seed.centroid, &other.centroid) {
                score += W_SOUND * embeddings::cosine(a, b).max(0.0);
            }
            (score > 0.0).then_some((*id, score))
        })
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    scored.truncate(k);
    Ok(scored)
}

// ----------------------------------------------------------------- the same --

/// How far two durations may drift and still be one recording. Encoder padding
/// and a lossy round-trip move the length by fractions of a second; two seconds
/// is generous and still rules out a radio edit against an album cut.
const DUP_SECS: f64 = 2.0;

/// How alike two fingerprints must be to call them the same recording.
///
/// Hand-set against the centred fingerprint of [`crate::analysis`], where
/// unrelated tracks scatter across the range rather than bunching near 1. It is
/// the one number here that is a judgement call: too low and a remaster joins
/// its original, too high and a 128k rip stops matching its own FLAC. The
/// duration window does most of the filtering, so this only has to separate
/// "the same music" from "the same length".
const DUP_COSINE: f32 = 0.95;

/// One copy of a recording the library holds more than once.
#[derive(Debug, Clone, PartialEq)]
pub struct Dupe {
    pub item_id: i64,
    /// How alike this copy is to the one worth keeping, 0..1.
    pub confidence: f32,
    /// The best copy in its group, on quality rather than on filename.
    pub keep: bool,
}

/// The library's duplicate recordings, largest group first.
///
/// Tags are deliberately not consulted: a duplicate that shares a title is one
/// a filename sort would already have found, and the copies actually worth
/// finding are the ones whose tags disagree. What identifies a recording here
/// is its length and its spectrum.
///
/// Returns nothing for tracks the analysis pass has not fingerprinted, which
/// means an unanalysed library reports no duplicates rather than guessing at
/// them from tags.
pub async fn duplicates(pool: &SqlitePool) -> Result<Vec<Vec<Dupe>>> {
    let all = facts(pool).await?;
    let prints = prints(pool).await?;
    let quality = quality_by_item(pool).await?;

    // Only tracks with both a length and a fingerprint can be compared at all.
    let mut cand: Vec<(&Facts, &Vec<f32>)> = all
        .iter()
        .filter_map(|f| match (f.duration_s, prints.get(&f.item_id)) {
            (Some(d), Some(v)) if d > 0.0 && !v.is_empty() => Some((f, v)),
            _ => None,
        })
        .collect();
    if cand.len() < 2 {
        return Ok(Vec::new());
    }
    // Sorted by length, so the only candidates for a match are neighbours and
    // the comparison is a sliding window rather than every pair against every
    // other — which on a large library is the difference between a moment and
    // a minute.
    cand.sort_by(|a, b| {
        a.0.duration_s
            .unwrap_or(0.0)
            .total_cmp(&b.0.duration_s.unwrap_or(0.0))
    });

    let mut group: Vec<usize> = (0..cand.len()).collect();
    fn root(group: &mut Vec<usize>, mut i: usize) -> usize {
        while group[i] != i {
            group[i] = group[group[i]]; // halve the path as we walk it
            i = group[i];
        }
        i
    }
    let mut score: HashMap<(usize, usize), f32> = HashMap::new();
    for i in 0..cand.len() {
        let di = cand[i].0.duration_s.unwrap_or(0.0);
        for j in (i + 1)..cand.len() {
            if cand[j].0.duration_s.unwrap_or(0.0) - di > DUP_SECS {
                break; // sorted: everything past here is longer still
            }
            let c = embeddings::cosine(cand[i].1, cand[j].1);
            if c >= DUP_COSINE {
                let (a, b) = (root(&mut group, i), root(&mut group, j));
                if a != b {
                    group[a] = b;
                }
                score.insert((i, j), c);
                score.insert((j, i), c);
            }
        }
    }

    let mut members: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..cand.len() {
        let r = root(&mut group, i);
        members.entry(r).or_default().push(i);
    }

    let mut out: Vec<Vec<Dupe>> = members
        .into_values()
        .filter(|g| g.len() > 1)
        .map(|g| {
            // The keeper is the best copy on quality. `max_by_key` returns the
            // last maximum, so ties fall to the highest item id — arbitrary, but
            // stable, which matters more than which of two identical rips wins.
            let best = *g
                .iter()
                .max_by_key(|i| quality.get(&cand[**i].0.item_id).copied().unwrap_or(0))
                .expect("a group has at least two members");
            g.into_iter()
                .map(|i| Dupe {
                    item_id: cand[i].0.item_id,
                    confidence: if i == best {
                        1.0
                    } else {
                        // Not always a direct comparison: A matched B and B
                        // matched C without A ever being compared to C, so fall
                        // back to measuring against the keeper here.
                        score
                            .get(&(i, best))
                            .copied()
                            .unwrap_or_else(|| embeddings::cosine(cand[i].1, cand[best].1))
                            .clamp(0.0, 1.0)
                    },
                    keep: i == best,
                })
                .collect()
        })
        .collect();
    out.sort_by(|a, b| b.len().cmp(&a.len()));
    Ok(out)
}


/// Albums that are another pressing of `album_id`, by the same test [`duplicates`]
/// uses: same length, same spectrum.
///
/// A deluxe edition, a remaster and a second rip of the same CD all end up as
/// separate albums in a library that was tagged by three different people, and
/// nothing connects them. Tags are not consulted, for the same reason as
/// there: disagreeing tags are what makes the pairing worth finding.
///
/// A *majority* of this album's tracks must match, not one. A compilation that
/// happens to carry one of these songs is not another edition of this record,
/// and treating it as one would put every greatest-hits album on every page.
pub async fn other_editions(pool: &SqlitePool, album_id: i64) -> Result<Vec<i64>> {
    let all = facts(pool).await?;
    let prints = prints(pool).await?;

    let albums: HashMap<i64, i64> = sqlx::query_as::<_, (i64, i64)>(
        "SELECT item_id, album_id FROM track_meta WHERE album_id IS NOT NULL",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .collect();

    let mine: Vec<(&Facts, &Vec<f32>)> = all
        .iter()
        .filter(|f| albums.get(&f.item_id) == Some(&album_id))
        .filter_map(|f| Some((f, prints.get(&f.item_id)?)))
        .filter(|(f, v)| f.duration_s.unwrap_or(0.0) > 0.0 && !v.is_empty())
        .collect();
    if mine.is_empty() {
        return Ok(Vec::new());
    }
    let need = (mine.len().div_ceil(2)).max(2);

    // How many of this album's tracks each other album has a copy of. Counted
    // per track of ours, so an album carrying the same song twice cannot reach
    // the threshold on its own.
    let mut hits: HashMap<i64, usize> = HashMap::new();
    for (f, v) in &mine {
        let dur = f.duration_s.unwrap_or(0.0);
        let mut matched: HashSet<i64> = HashSet::new();
        for other in &all {
            let Some(theirs) = albums.get(&other.item_id).copied() else { continue };
            if theirs == album_id {
                continue;
            }
            if (other.duration_s.unwrap_or(0.0) - dur).abs() > DUP_SECS {
                continue;
            }
            let Some(ov) = prints.get(&other.item_id) else { continue };
            if embeddings::cosine(v, ov) >= DUP_COSINE {
                matched.insert(theirs);
            }
        }
        for a in matched {
            *hits.entry(a).or_insert(0) += 1;
        }
    }

    let mut out: Vec<(i64, usize)> =
        hits.into_iter().filter(|(_, n)| *n >= need).collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    Ok(out.into_iter().map(|(id, _)| id).collect())
}

/// A rank for "which copy is worth keeping", high is better.
///
/// Lossless always beats lossy, whatever the bitrate says — a 320k MP3 reports
/// a bigger number than a FLAC's absent one, and keeping the MP3 over the FLAC
/// is precisely the mistake this exists to avoid.
async fn quality_by_item(pool: &SqlitePool) -> Result<HashMap<i64, i64>> {
    let rows: Vec<(i64, Option<String>, Option<i64>, Option<i64>)> = sqlx::query_as(
        "SELECT item_id, codec, bitrate, sample_rate FROM track_meta",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, codec, bitrate, rate)| {
            const LOSSLESS: [&str; 7] = ["flac", "alac", "wav", "aiff", "aif", "ape", "wv"];
            let c = codec.unwrap_or_default().to_ascii_lowercase();
            let lossless = LOSSLESS.iter().any(|l| c.contains(l));
            let rank = if lossless {
                1_000_000 + rate.unwrap_or(0)
            } else {
                bitrate.unwrap_or(0)
            };
            (id, rank)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::{add_track, open_pool};

    /// A unit vector that leans on one axis, so two calls with nearby `lean`
    /// values are similar and distant ones are not.
    fn print_at(lean: usize, dim: usize) -> Vec<f32> {
        let mut v = vec![0.0f32; dim];
        v[lean % dim] = 1.0;
        v[(lean + 1) % dim] = 0.5;
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.into_iter().map(|x| x / n).collect()
    }

    async fn track(pool: &SqlitePool, path: &str, genre: &str, year: i64, dur: f64) -> i64 {
        let id = add_track(pool, path).await;
        sqlx::query("UPDATE track_meta SET genre = ?, year = ?, duration_s = ? WHERE item_id = ?")
            .bind(genre)
            .bind(year)
            .bind(dur)
            .bind(id)
            .execute(pool)
            .await
            .unwrap();
        id
    }

    #[tokio::test]
    async fn a_station_prefers_the_nearer_sound_and_spreads_the_artists() {
        let (_t, pool) = open_pool().await;
        let seed = track(&pool, "/m/seed.flac", "ambient", 2000, 200.0).await;
        let near = track(&pool, "/m/near.flac", "ambient", 2001, 200.0).await;
        let far = track(&pool, "/m/far.flac", "grindcore", 1975, 90.0).await;
        for (id, lean) in [(seed, 0), (near, 0), (far, 9)] {
            embeddings::store(&pool, id, analysis::FINGERPRINT_MODEL, &print_at(lean, 20))
                .await
                .unwrap();
        }
        let run = station(&pool, &[seed], &[], 10).await.unwrap();
        assert_eq!(run.first(), Some(&near), "the nearest sound leads");
        assert!(!run.contains(&seed), "a station never offers its own seed");

        // Everything by one artist: the cap must hold even when they all match.
        let (_t2, pool2) = open_pool().await;
        let mut ids = Vec::new();
        for i in 0..6 {
            let id = track(&pool2, &format!("/m/{i}.flac"), "ambient", 2000, 200.0).await;
            sqlx::query("UPDATE track_meta SET artist_id = 1 WHERE item_id = ?")
                .bind(id)
                .execute(&pool2)
                .await
                .unwrap();
            ids.push(id);
        }
        let run = station(&pool2, &[ids[0]], &[], 10).await.unwrap();
        assert_eq!(run.len(), MAX_PER_ARTIST, "one artist cannot fill a station");
    }

    #[tokio::test]
    async fn an_excluded_track_stays_out() {
        let (_t, pool) = open_pool().await;
        let seed = track(&pool, "/m/s.flac", "dub", 1998, 300.0).await;
        let queued = track(&pool, "/m/q.flac", "dub", 1998, 300.0).await;
        let free = track(&pool, "/m/f.flac", "dub", 1998, 300.0).await;
        let run = station(&pool, &[seed], &[queued], 10).await.unwrap();
        assert_eq!(run, vec![free]);
    }

    #[tokio::test]
    async fn the_same_recording_twice_is_found_and_the_flac_is_kept() {
        let (_t, pool) = open_pool().await;
        // Same music, same length, different rips — and deliberately different
        // titles, because tags are what cannot be trusted here.
        let flac = track(&pool, "/m/a.flac", "", 0, 355.0).await;
        let mp3 = track(&pool, "/m/a.mp3", "", 0, 355.4).await;
        let other = track(&pool, "/m/other.flac", "", 0, 356.0).await;
        sqlx::query("UPDATE track_meta SET codec='flac', sample_rate=44100 WHERE item_id IN (?,?)")
            .bind(flac)
            .bind(other)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE track_meta SET codec='mp3', bitrate=320000 WHERE item_id = ?")
            .bind(mp3)
            .execute(&pool)
            .await
            .unwrap();

        let same = print_at(0, 20);
        let nudged = {
            let mut v = same.clone();
            v[5] += 0.05; // a lossy rip, not a different song
            let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            v.into_iter().map(|x| x / n).collect::<Vec<f32>>()
        };
        embeddings::store(&pool, flac, analysis::FINGERPRINT_MODEL, &same).await.unwrap();
        embeddings::store(&pool, mp3, analysis::FINGERPRINT_MODEL, &nudged).await.unwrap();
        embeddings::store(&pool, other, analysis::FINGERPRINT_MODEL, &print_at(9, 20)).await.unwrap();

        let groups = duplicates(&pool).await.unwrap();
        assert_eq!(groups.len(), 1, "one duplicate pair, and the third is its own");
        let g = &groups[0];
        assert_eq!(g.len(), 2);
        let keeper = g.iter().find(|d| d.keep).expect("a group has a keeper");
        assert_eq!(keeper.item_id, flac, "lossless outranks a 320k mp3");
        assert!(g.iter().all(|d| d.confidence > 0.9));
    }

    #[tokio::test]
    async fn another_edition_needs_most_of_the_record_not_one_song() {
        let (_t, pool) = open_pool().await;
        sqlx::query("INSERT INTO albums (id, title) VALUES (1,'Original'), (2,'Remaster'), (3,'Hits')")
            .execute(&pool).await.unwrap();
        // Four tracks, three of which the remaster also has; the compilation
        // carries exactly one of them.
        let mut mine = Vec::new();
        for n in 0..4 {
            let id = track(&pool, &format!("/m/a{n}.flac"), "", 0, 200.0 + n as f64).await;
            sqlx::query("UPDATE track_meta SET album_id = 1 WHERE item_id = ?")
                .bind(id).execute(&pool).await.unwrap();
            embeddings::store(&pool, id, analysis::FINGERPRINT_MODEL, &print_at(n, 20))
                .await.unwrap();
            mine.push(id);
        }
        for n in 0..3 {
            let id = track(&pool, &format!("/m/b{n}.flac"), "", 0, 200.0 + n as f64).await;
            sqlx::query("UPDATE track_meta SET album_id = 2 WHERE item_id = ?")
                .bind(id).execute(&pool).await.unwrap();
            embeddings::store(&pool, id, analysis::FINGERPRINT_MODEL, &print_at(n, 20))
                .await.unwrap();
        }
        let one = track(&pool, "/m/c0.flac", "", 0, 200.0).await;
        sqlx::query("UPDATE track_meta SET album_id = 3 WHERE item_id = ?")
            .bind(one).execute(&pool).await.unwrap();
        embeddings::store(&pool, one, analysis::FINGERPRINT_MODEL, &print_at(0, 20))
            .await.unwrap();

        let editions = other_editions(&pool, 1).await.unwrap();
        assert_eq!(editions, vec![2], "the compilation shares one song, not the record");
        // And it works the other way round: three of three match.
        assert_eq!(other_editions(&pool, 2).await.unwrap(), vec![1]);
        // An album nobody has a second copy of has no other editions.
        assert!(other_editions(&pool, 3).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_unanalysed_library_reports_no_duplicates() {
        let (_t, pool) = open_pool().await;
        track(&pool, "/m/a.flac", "", 0, 200.0).await;
        track(&pool, "/m/b.flac", "", 0, 200.0).await;
        assert!(duplicates(&pool).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn similar_artists_rank_by_shared_ground() {
        let (_t, pool) = open_pool().await;
        // 1 = the seed, 2 shares its genre and era, 3 shares nothing.
        for (artist, path, genre, year) in [
            (1i64, "/m/1.flac", "trip-hop", 1997),
            (2, "/m/2.flac", "Trip-Hop", 1998),
            (3, "/m/3.flac", "bluegrass", 1962),
        ] {
            let id = track(&pool, path, genre, year, 200.0).await;
            sqlx::query("UPDATE track_meta SET artist_id = ? WHERE item_id = ?")
                .bind(artist)
                .bind(id)
                .execute(&pool)
                .await
                .unwrap();
        }
        let near = similar_artists(&pool, 1, 5).await.unwrap();
        assert_eq!(near.first().map(|(id, _)| *id), Some(2));
        assert!(!near.iter().any(|(id, _)| *id == 1), "an artist is not their own match");
    }
}
