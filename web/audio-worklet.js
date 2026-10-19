// Plays the client's mix from a ring in its shared wasm memory (crates/benilla-app/src/sound/output/web.rs),
// on the audio thread, so a busy main thread cannot starve it.
class BenillaRing extends AudioWorkletProcessor {
    constructor(options) {
        super();
        const o = options.processorOptions;
        this.memory = o.memory;
        this.read = o.read >> 2;
        this.write = o.write >> 2;
        this.samples = o.samples;
        this.frames = o.frames;
        // Quanta played, and quanta that ran dry after the first sound: the stutter meter.
        this.quanta = 0;
        this.underruns = 0;
        this.started = false;
    }

    process(_inputs, outputs) {
        // A grown memory is a new buffer over the same bytes; take the current one.
        const buffer = this.memory.buffer;
        const idx = new Int32Array(buffer);
        const samples = new Float32Array(buffer, this.samples, this.frames * 2);
        const [left, right] = outputs[0];
        let read = Atomics.load(idx, this.read) >>> 0;
        const write = Atomics.load(idx, this.write) >>> 0;
        if (read !== write) this.started = true;
        if (this.started && ((write - read) >>> 0) < left.length) this.underruns++;
        if (++this.quanta % 375 === 0) this.port.postMessage({ quanta: this.quanta, underruns: this.underruns });
        for (let i = 0; i < left.length; i++) {
            if (read === write) {
                left[i] = right[i] = 0;
                continue;
            }
            const at = (read % this.frames) * 2;
            left[i] = samples[at];
            right[i] = samples[at + 1];
            read = (read + 1) >>> 0;
        }
        Atomics.store(idx, this.read, read | 0);
        return true;
    }
}

registerProcessor('benilla-ring', BenillaRing);
