//! The web half of [`super`]: the surface `cpal.rs` offers, over an AudioWorklet. The worklet
//! (`web/audio-worklet.js`) plays from a ring in the shared wasm memory on the browser's audio
//! thread, and a worker runs the cycle callback whenever the ring has room, so neither waits on the
//! page's main thread, which a loading screen keeps busy for seconds at a time.

use std::cell::{OnceCell, UnsafeCell};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use web_time::Instant;

use anyhow::{anyhow, bail, Result};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

pub(super) const CHANNELS: u32 = 2;

/// Frames the worker renders per cycle.
const BLOCK_FRAMES: usize = 256;
/// The ring's size, frames: about 85 ms at 48 kHz, a power of two for the index arithmetic.
const RING_FRAMES: usize = 4096;
/// How often the worker checks the ring for room.
const PUMP_EVERY: Duration = Duration::from_millis(2);

pub(super) fn now_ns() -> u64 {
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

thread_local! {
    static CONTEXT: OnceCell<Option<web_sys::AudioContext>> = const { OnceCell::new() };
}

/// The rate the page's AudioContext runs at, for threads that cannot reach the context.
static SAMPLE_RATE: AtomicU32 = AtomicU32::new(0);

/// The page's one AudioContext, made on first use on the main thread; the page resumes it on the
/// first click or key, as browsers start audio suspended.
fn context() -> Option<web_sys::AudioContext> {
    CONTEXT.with(|c| {
        c.get_or_init(|| {
            let ctx = web_sys::AudioContext::new().ok()?;
            SAMPLE_RATE.store(ctx.sample_rate() as u32, Ordering::Release);
            Some(ctx)
        })
        .clone()
    })
}

#[derive(Clone, Debug)]
pub(super) struct Device {
    pub name: String,
    pub sample_rate: u32,
    pub buffer_range: (u32, u32),
    pub latency_frames: u32,
    pub safety_frames: u32,
}

pub(super) fn default_output() -> Result<Device> {
    now_ns();
    let sample_rate = match context() {
        Some(ctx) => ctx.sample_rate() as u32,
        None => SAMPLE_RATE.load(Ordering::Acquire),
    };
    if sample_rate == 0 {
        bail!("no AudioContext");
    }
    Ok(Device {
        name: "Web Audio".into(),
        sample_rate,
        buffer_range: (BLOCK_FRAMES as u32, BLOCK_FRAMES as u32),
        latency_frames: 0,
        safety_frames: 0,
    })
}

pub(super) struct Cycle<'a> {
    pub buffer: &'a mut [f32],
    pub frames: usize,
    pub output_time_ns: u64,
}

/// Interleaved stereo frames between the worker (writes) and the worklet (reads). The indices
/// count frames and wrap; the worklet reads and stores `read` with `Atomics`.
#[repr(C)]
struct Ring {
    read: AtomicU32,
    write: AtomicU32,
    samples: UnsafeCell<[f32; RING_FRAMES * CHANNELS as usize]>,
}

// SAFETY: one writer (the worker) and one reader (the worklet) touch disjoint frames, ordered by
// the indices' acquire and release.
unsafe impl Sync for Ring {}

pub(super) struct Stream {
    stop: Arc<AtomicBool>,
    _ring: Arc<Ring>,
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

impl Stream {
    pub(super) fn buffer_frames(&self) -> u32 {
        BLOCK_FRAMES as u32
    }

    pub(super) fn open<F>(
        device: &Device,
        _buffer_frames: u32,
        _notices: &Arc<Notices>,
        mut on_cycle: F,
    ) -> Result<Self>
    where
        F: FnMut(Cycle<'_>) + Send + 'static,
    {
        let ctx = context().ok_or_else(|| anyhow!("no AudioContext"))?;
        let ring = Arc::new(Ring {
            read: AtomicU32::new(0),
            write: AtomicU32::new(0),
            samples: UnsafeCell::new([0.0; RING_FRAMES * CHANNELS as usize]),
        });
        connect_worklet(&ctx, &ring);

        let stop = Arc::new(AtomicBool::new(false));
        let (pump_ring, pump_stop) = (Arc::clone(&ring), Arc::clone(&stop));
        let rate = u64::from(device.sample_rate.max(1));
        wasm_thread::Builder::new()
            .name("audio-pump".into())
            .spawn(move || {
                let mut block = vec![0f32; BLOCK_FRAMES * CHANNELS as usize];
                while !pump_stop.load(Ordering::Acquire) {
                    let write = pump_ring.write.load(Ordering::Relaxed);
                    let queued = write.wrapping_sub(pump_ring.read.load(Ordering::Acquire));
                    if (RING_FRAMES as u32).saturating_sub(queued) < BLOCK_FRAMES as u32 {
                        std::thread::sleep(PUMP_EVERY);
                        continue;
                    }
                    on_cycle(Cycle {
                        buffer: &mut block,
                        frames: BLOCK_FRAMES,
                        output_time_ns: now_ns() + u64::from(queued) * 1_000_000_000 / rate,
                    });
                    // SAFETY: frames `write..write + BLOCK` are not yet published, so the worklet
                    // does not read them.
                    let samples = unsafe { &mut *pump_ring.samples.get() };
                    for (i, frame) in block.chunks_exact(2).enumerate() {
                        let at = (write as usize + i) % RING_FRAMES * 2;
                        samples[at..at + 2].copy_from_slice(frame);
                    }
                    pump_ring
                        .write
                        .store(write.wrapping_add(BLOCK_FRAMES as u32), Ordering::Release);
                }
            })
            .map_err(|e| anyhow!("spawning the audio pump: {e}"))?;
        Ok(Self { stop, _ring: ring })
    }
}

/// Load the worklet module and attach a node that plays `ring`, once the module is in.
fn connect_worklet(ctx: &web_sys::AudioContext, ring: &Arc<Ring>) {
    let Ok(worklet) = ctx.audio_worklet() else {
        bevy::log::warn!("audio: no AudioWorklet");
        return;
    };
    let Ok(loaded) = worklet.add_module("./audio-worklet.js") else {
        bevy::log::warn!("audio: cannot load the worklet module");
        return;
    };
    let options = js_sys::Object::new();
    let set = |k: &str, v: &JsValue| {
        let _ = js_sys::Reflect::set(&options, &k.into(), v);
    };
    set("memory", &wasm_bindgen::memory());
    set("read", &(std::ptr::from_ref(&ring.read) as usize).into());
    set("write", &(std::ptr::from_ref(&ring.write) as usize).into());
    set("samples", &(ring.samples.get() as usize).into());
    set("frames", &(RING_FRAMES as u32).into());
    let ctx = ctx.clone();
    let attach = Closure::once(move |_: JsValue| {
        let node_options = web_sys::AudioWorkletNodeOptions::new();
        node_options.set_processor_options(Some(&options));
        node_options.set_output_channel_count(&js_sys::Array::of1(&2.into()));
        match web_sys::AudioWorkletNode::new_with_options(&ctx, "benilla-ring", &node_options) {
            Ok(node) => {
                let _ = node.connect_with_audio_node(&ctx.destination());
            }
            Err(e) => web_sys::console::error_2(&"audio: worklet node failed".into(), &e),
        }
    });
    let _ = loaded.then(&attach);
    attach.forget();
}

#[derive(Default)]
pub(super) struct Notices {
    pub default_changed: AtomicBool,
    pub device_died: AtomicBool,
    pub rate_changed: AtomicBool,
    pub overloads: AtomicU64,
    pub last_overload_ns: AtomicU64,
}

/// The browser picks the output device; there is nothing to watch.
pub(super) struct Listeners;

impl Listeners {
    pub(super) fn arm(_device: &Device, _notices: Arc<Notices>) -> Self {
        Self
    }
}

pub(super) struct Workgroup;

impl Workgroup {
    pub(super) fn of_device(_device: &Device) -> Option<Self> {
        None
    }
}

pub(super) struct Joined;

impl Joined {
    pub(super) fn join(_group: Workgroup) -> Result<Self> {
        bail!("no audio workgroups on the web")
    }
}

pub(super) struct Realtime;

pub(super) fn set_realtime(_period_ns: u64) -> Result<Realtime> {
    bail!("no realtime thread policy on the web")
}
