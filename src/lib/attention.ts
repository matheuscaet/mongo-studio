/**
 * The chime that plays when something waits for the user (an MCP client's
 * request set to Ask). Bringing the window forward is the backend's job
 * (src-tauri/src/attention.rs): a minimized window's webview may be
 * suspended when the request arrives.
 */

let context: AudioContext | null = null;
let lastChime = 0;

function audio(): AudioContext {
  context ??= new AudioContext();
  return context;
}

/**
 * Webviews start audio suspended until the user interacts with the page,
 * and a request usually arrives while they're elsewhere: resume it on the
 * first click or key press, so the chime can play later.
 */
export function unlockAudioOnFirstGesture() {
  const unlock = () => {
    void audio().resume().catch(() => {});
    window.removeEventListener("pointerdown", unlock, true);
    window.removeEventListener("keydown", unlock, true);
  };
  window.addEventListener("pointerdown", unlock, true);
  window.addEventListener("keydown", unlock, true);
}

/** Two soft rising notes, at most once a second and a half. */
export async function playChime(force = false) {
  const now = Date.now();
  if (!force && now - lastChime < 1500) return;
  lastChime = now;
  try {
    const ctx = audio();
    if (ctx.state === "suspended") await ctx.resume();
    const start = ctx.currentTime + 0.02;
    for (const [frequency, offset] of [
      [659.25, 0], // E5
      [987.77, 0.14], // B5
    ]) {
      const tone = ctx.createOscillator();
      tone.type = "sine";
      tone.frequency.value = frequency;
      const level = ctx.createGain();
      const at = start + offset;
      level.gain.setValueAtTime(0.0001, at);
      level.gain.exponentialRampToValueAtTime(0.2, at + 0.015);
      level.gain.exponentialRampToValueAtTime(0.0001, at + 0.42);
      tone.connect(level).connect(ctx.destination);
      tone.start(at);
      tone.stop(at + 0.45);
    }
  } catch {
    // no audio device, or audio blocked: the backend still calls the user
  }
}
