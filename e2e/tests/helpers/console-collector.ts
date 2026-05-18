import type { Page, ConsoleMessage } from "@playwright/test";

/** Patterns that indicate a WASM panic or fatal runtime error. */
const PANIC_PATTERNS = [
  "panicked at",
  "unreachable",
  "time not implemented",
  "RuntimeError:",
  "wasm-bindgen:",
  "failed to create",
  "out of memory",
] as const;

/** Messages we expect and deliberately ignore. */
const IGNORED_PATTERNS = [
  // Trunk hot-reload WebSocket fails when served by our custom server
  "WebSocket",
  // Browser noise
  "favicon.ico",
  // WASM threading noise: rayon worker threads occasionally hit signature
  // mismatches during SharedArrayBuffer handshake, then retry successfully
  "function signature mismatch",
] as const;

export interface CollectedError {
  text: string;
  type: "console.error" | "pageerror" | "bevy.error";
}

/**
 * Bevy's web build routes the `tracing` macros through `tracing-wasm`, which
 * emits *every* level (TRACE..ERROR) via `console.log` with CSS styling — the
 * level is encoded in the message text as `%cERROR%c`/`%cWARN%c`/etc., not in
 * the Playwright `msg.type()`. Filtering by `type === "error"` therefore loses
 * every Bevy-side ERROR-level log (including fatal runtime errors like
 * `DeviceLost`). We detect them by parsing the styled marker out of the
 * message text instead.
 */
const BEVY_ERROR_MARKER = "%cERROR%c";

/**
 * Attaches to a Playwright page and collects console errors + uncaught exceptions.
 * Use `hasPanic` / `firstPanic` for quick assertions in tests.
 */
export class ConsoleCollector {
  readonly errors: CollectedError[] = [];
  private _firstPanic: string | null = null;

  /** Start collecting. Call before page.goto(). */
  attach(page: Page): void {
    page.on("console", (msg: ConsoleMessage) => {
      const text = msg.text();
      // Chromium reports failed resource loads (e.g. the favicon.ico the
      // browser fetches itself with no element in the HTML) as a generic
      // `console.error` whose text is "Failed to load resource: …" — the
      // URL is only in `msg.location().url`. We have to inspect both
      // sources to filter that noise.
      const locUrl = msg.location()?.url ?? "";
      if (this.isIgnored(text) || this.isIgnored(locUrl)) return;
      const isBevyError =
        msg.type() === "log" && text.includes(BEVY_ERROR_MARKER);
      if (msg.type() !== "error" && !isBevyError) return;
      const type = isBevyError ? "bevy.error" : "console.error";
      this.errors.push({ text, type });
      if (!this._firstPanic && this.isPanic(text)) {
        this._firstPanic = text;
      }
    });

    page.on("pageerror", (error: Error) => {
      const text = error.message;
      if (this.isIgnored(text)) return;
      this.errors.push({ text, type: "pageerror" });
      if (!this._firstPanic && this.isPanic(text)) {
        this._firstPanic = text;
      }
    });
  }

  get hasPanic(): boolean {
    return this._firstPanic !== null;
  }

  get firstPanic(): string | null {
    return this._firstPanic;
  }

  /** Non-panic errors (warnings, benign failures). */
  get nonPanicErrors(): CollectedError[] {
    return this.errors.filter((e) => !this.isPanic(e.text));
  }

  private isPanic(text: string): boolean {
    const lower = text.toLowerCase();
    return PANIC_PATTERNS.some((p) => lower.includes(p.toLowerCase()));
  }

  private isIgnored(text: string): boolean {
    return IGNORED_PATTERNS.some((p) => text.includes(p));
  }
}
