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
  type: "console.error" | "pageerror";
}

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
      if (msg.type() !== "error") return;
      const text = msg.text();
      if (this.isIgnored(text)) return;
      this.errors.push({ text, type: "console.error" });
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
