// The few `wasi_snapshot_preview1` calls the Lua VM's libc makes: no environment, stdout and
// stderr to the console, every file refused.
// Workers' consoles are not the page's: mirror them to it (web/index.html re-logs them).
if (typeof WorkerGlobalScope !== 'undefined' && !globalThis.__benillaLogMirror) {
    globalThis.__benillaLogMirror = new BroadcastChannel('benilla-worker-log');
    for (const level of ['log', 'info', 'warn', 'error']) {
        const real = console[level].bind(console);
        console[level] = (...args) => {
            real(...args);
            try { globalThis.__benillaLogMirror.postMessage({ level, text: args.map(String).join(' ') }); } catch (_) {}
        };
    }
}

// The bindings publish the module's shared memory here on every thread (scripts/web-build.sh).
const memory = { get buffer() { return globalThis.__benillaMemory.buffer; } };
const view = () => new DataView(memory.buffer);
const ENOSYS = 52, EBADF = 8;
const text = new TextDecoder();
const lines = { 1: '', 2: '' };

export function environ_sizes_get(count, size) { view().setUint32(count, 0, true); view().setUint32(size, 0, true); return 0; }
export function environ_get() { return 0; }
export function clock_time_get(_id, _precision, out) {
    view().setBigUint64(out, BigInt(Math.round(performance.timeOrigin + performance.now())) * 1000000n, true);
    return 0;
}
export function fd_write(fd, iovs, count, written) {
    let total = 0;
    for (let i = 0; i < count; i++) {
        const ptr = view().getUint32(iovs + 8 * i, true), len = view().getUint32(iovs + 8 * i + 4, true);
        if (fd === 1 || fd === 2) lines[fd] += text.decode(new Uint8Array(memory.buffer, ptr, len).slice());
        total += len;
    }
    for (const fd of [1, 2]) {
        const parts = lines[fd].split('\n');
        lines[fd] = parts.pop();
        for (const p of parts) (fd === 1 ? console.log : console.warn)('[lua]', p);
    }
    view().setUint32(written, total, true);
    return 0;
}
export function fd_prestat_get() { return EBADF; }
export function fd_prestat_dir_name() { return EBADF; }
export function sched_yield() { return 0; }
export function proc_exit(code) { throw new Error(`wasi proc_exit(${code})`); }
export const fd_close = () => EBADF, fd_fdstat_get = () => EBADF, fd_fdstat_set_flags = () => EBADF,
    fd_read = () => EBADF, fd_renumber = () => EBADF, fd_seek = () => EBADF, path_open = () => ENOSYS,
    path_remove_directory = () => ENOSYS, path_rename = () => ENOSYS, path_unlink_file = () => ENOSYS;
