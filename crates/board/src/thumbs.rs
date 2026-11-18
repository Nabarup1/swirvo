//! Thumbnail service (docs/12 §3.4): worker threads decode off the UI thread; the UI asks
//! for what's visible each frame and only that stays queued; results land in an LRU.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};

use swirvo_ui::{Pixmap, Waker};

/// Decodes a file into a straight-alpha RGBA thumbnail no larger than `max` px.
pub type Decoder = Arc<dyn Fn(&Path, u32) -> Option<image::RgbaImage> + Send + Sync>;

type Key = (PathBuf, u32);

struct Queue {
    jobs: VecDeque<Key>,
    quit: bool,
}

struct Shared {
    q: Mutex<Queue>,
    cv: Condvar,
}

pub struct Thumbs {
    shared: Arc<Shared>,
    rx: Receiver<(Key, Option<image::RgbaImage>)>,
    cache: HashMap<Key, (Option<Pixmap>, u64)>,
    bytes: usize,
    budget: usize,
    tick: u64,
    in_flight: HashSet<Key>,
    wanted: Vec<Key>,
}

/// Straight RGBA → premultiplied pixmap.
pub fn to_pixmap(img: &image::RgbaImage) -> Option<Pixmap> {
    let (w, h) = img.dimensions();
    let mut data = img.as_raw().clone();
    for p in data.as_chunks_mut::<4>().0 {
        let a = p[3] as u32;
        if a < 255 {
            for c in &mut p[..3] {
                *c = ((*c as u32 * a + 127) / 255) as u8;
            }
        }
    }
    Pixmap::from_vec(data, tiny_size(w, h)?)
}

fn tiny_size(w: u32, h: u32) -> Option<swirvo_ui::PixSize> {
    swirvo_ui::PixSize::from_wh(w, h)
}

impl Thumbs {
    pub fn new(decoder: Decoder, waker: Waker, workers: usize) -> Thumbs {
        let shared = Arc::new(Shared {
            q: Mutex::new(Queue {
                jobs: VecDeque::new(),
                quit: false,
            }),
            cv: Condvar::new(),
        });
        let (tx, rx) = channel();
        for _ in 0..workers.max(1) {
            let (shared, tx, dec, wake): (Arc<Shared>, Sender<_>, Decoder, Waker) =
                (shared.clone(), tx.clone(), decoder.clone(), waker.clone());
            std::thread::spawn(move || {
                loop {
                    let job = {
                        let mut q = shared.q.lock().unwrap_or_else(|p| p.into_inner());
                        loop {
                            if q.quit {
                                return;
                            }
                            if let Some(j) = q.jobs.pop_front() {
                                break j;
                            }
                            q = shared.cv.wait(q).unwrap_or_else(|p| p.into_inner());
                        }
                    };
                    // A panicking decoder must not take the worker down.
                    let img = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        dec(&job.0, job.1)
                    }))
                    .ok()
                    .flatten();
                    if tx.send((job, img)).is_err() {
                        return;
                    }
                    wake();
                }
            });
        }
        Thumbs {
            shared,
            rx,
            cache: HashMap::new(),
            bytes: 0,
            budget: 32 << 20,
            tick: 0,
            in_flight: HashSet::new(),
            wanted: Vec::new(),
        }
    }

    /// Take finished thumbnails. Returns true if any arrived (repaint).
    pub fn poll(&mut self) -> bool {
        let mut any = false;
        while let Ok((key, img)) = self.rx.try_recv() {
            self.in_flight.remove(&key);
            let pm = img.as_ref().and_then(to_pixmap);
            self.bytes += pm.as_ref().map_or(64, |p| p.data().len());
            self.tick += 1;
            self.cache.insert(key, (pm, self.tick));
            any = true;
        }
        self.evict();
        any
    }

    fn evict(&mut self) {
        while self.bytes > self.budget && self.cache.len() > 1 {
            let Some(oldest) = self
                .cache
                .iter()
                .min_by_key(|(_, (_, t))| *t)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            if let Some((pm, _)) = self.cache.remove(&oldest) {
                self.bytes = self.bytes.saturating_sub(pm.map_or(64, |p| p.data().len()));
            }
        }
    }

    /// The thumbnail if ready; otherwise it's requested for this frame. `Some(None)` means
    /// "tried, no thumbnail for this file" (show the type badge).
    pub fn get(&mut self, path: &Path, max: u32) -> Option<Option<&Pixmap>> {
        let key = (path.to_path_buf(), max);
        self.tick += 1;
        let tick = self.tick;
        if let Some(e) = self.cache.get_mut(&key) {
            e.1 = tick;
            return Some(e.0.as_ref());
        }
        if !self.in_flight.contains(&key) {
            self.wanted.push(key);
        }
        None
    }

    /// Replace the queue with this frame's requests (visible items, in visual order).
    pub fn end_frame(&mut self) {
        let wanted = std::mem::take(&mut self.wanted);
        let mut q = self.shared.q.lock().unwrap_or_else(|p| p.into_inner());
        // Jobs still queued (not started) that are no longer visible are dropped.
        for k in q.jobs.drain(..) {
            self.in_flight.remove(&k);
        }
        for k in wanted {
            if self.in_flight.insert(k.clone()) {
                q.jobs.push_back(k);
            }
        }
        drop(q);
        self.shared.cv.notify_all();
    }

    pub fn busy(&self) -> bool {
        !self.in_flight.is_empty()
    }
}

impl Drop for Thumbs {
    fn drop(&mut self) {
        let mut q = self.shared.q.lock().unwrap_or_else(|p| p.into_inner());
        q.quit = true;
        q.jobs.clear();
        drop(q);
        self.shared.cv.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_off_thread_and_caches() {
        let dec: Decoder = Arc::new(|p: &Path, max: u32| {
            if p.ends_with("none") {
                None
            } else {
                Some(image::RgbaImage::from_pixel(
                    max,
                    max / 2,
                    image::Rgba([255, 0, 0, 128]),
                ))
            }
        });
        let mut t = Thumbs::new(dec, Arc::new(|| {}), 2);
        let (a, b) = (PathBuf::from("a.png"), PathBuf::from("none"));
        assert!(t.get(&a, 64).is_none());
        assert!(t.get(&b, 64).is_none());
        t.end_frame();
        for _ in 0..200 {
            t.poll();
            if !t.busy() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let pm = t
            .get(&a, 64)
            .flatten()
            .map(|p| (p.width(), p.height(), p.pixel(0, 0)));
        let (w, h, px) = pm.unwrap();
        assert_eq!((w, h), (64, 32));
        // Premultiplied: red 255 at alpha 128 → 128.
        assert_eq!(px.map(|p| (p.red(), p.alpha())), Some((128, 128)));
        assert!(matches!(t.get(&b, 64), Some(None)));
    }
}
