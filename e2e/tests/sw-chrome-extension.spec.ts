// Repro for the SW caching bug:
//
//   sw.js:75 Uncaught (in promise) TypeError: Failed to execute 'put' on
//   'Cache': Request scheme 'chrome-extension' is unsupported
//       at cacheFirst (sw.js:75:11)
//
// Trigger: a browser extension (content script, etc.) issues a fetch whose URL
// has scheme `chrome-extension://...` and a pathname ending in `.js`/`.wasm`.
// The SW's fetch listener routes it through `cacheFirst`, which calls
// `cache.put(request, response)`. The Cache API rejects non-http(s) schemes
// with the TypeError above. Since the `cache.put(...)` call is not awaited,
// the rejection surfaces as an unhandled promise rejection (the "Uncaught (in
// promise)" log line the user sees).
//
// The SW only ships in production (init.js installs it; trunk serve does
// not), so we can't drive the bug by registering the real SW in the test
// server. Instead, we load the sw.js source verbatim, evaluate it inside the
// page so the `caches` global is the real Cache API for the page's origin,
// capture the fetch handler it registers, and synthesize a FetchEvent with a
// chrome-extension Request. `fetch` is stubbed so the handler reaches the
// `cache.put` call.

import * as fs from "node:fs/promises";
import * as path from "node:path";
import { fileURLToPath } from "node:url";
import { test, expect } from "@playwright/test";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const SW_PATH = path.resolve(
  __dirname,
  "../../crates/bevy_naadf/dist/sw.js",
);

interface ReproResult {
  respondWithCalled: boolean;
  responseDelivered: boolean;
  unhandledRejections: string[];
  caughtErrors: string[];
}

test.describe("Service worker chrome-extension cache bug", () => {
  // Regression test. Pre-fix: the SW's fetch handler matched any URL whose
  // pathname ended in `.js`/`.wasm`, routed it through `cacheFirst`, and the
  // unawaited `cache.put(...)` rejected with a TypeError naming the
  // `chrome-extension` scheme — the "Uncaught (in promise)" line in console.
  // Post-fix: the SW early-returns from the fetch listener for any non-http(s)
  // request, so `respondWith` is never called and no cache.put is attempted.
  test("does not intercept chrome-extension URLs", async ({ page }) => {
    const swSource = await fs.readFile(SW_PATH, "utf-8");

    // Need a real origin so `caches` is available.
    await page.goto("/", { waitUntil: "commit" });

    const result = await page.evaluate(
      async (src): Promise<ReproResult> => {
        // Capture the fetch handler that sw.js registers via
        // `self.addEventListener('fetch', ...)`.
        let fetchHandler: ((event: unknown) => void) | null = null;
        const sandboxSelf = {
          addEventListener: (type: string, handler: (event: unknown) => void) => {
            if (type === "fetch") fetchHandler = handler;
          },
          skipWaiting: () => {},
          clients: { claim: () => Promise.resolve() },
        };

        // Evaluate sw.js with `self` rebound to our sandbox. `caches`,
        // `fetch`, `URL`, `Request`, `Response` keep their real bindings via
        // the global scope, so cache.put hits the real Cache API and throws
        // the genuine "scheme unsupported" TypeError.
        new Function("self", src)(sandboxSelf);

        if (!fetchHandler) {
          return {
            respondWithCalled: false,
            responseDelivered: false,
            unhandledRejections: ["no fetch handler captured from sw.js"],
            caughtErrors: [],
          };
        }

        // Capture the unhandled promise rejection. cache.put inside
        // cacheFirst is not awaited, so the TypeError surfaces here.
        const unhandledRejections: string[] = [];
        const onUnhandled = (e: PromiseRejectionEvent) => {
          unhandledRejections.push(String(e.reason));
          e.preventDefault();
        };
        window.addEventListener("unhandledrejection", onUnhandled);

        // Stub fetch to return a fake successful .js response so the handler
        // reaches the `cache.put(...)` line. The real network would refuse a
        // chrome-extension:// fetch from a page context.
        const realFetch = window.fetch;
        window.fetch = async () =>
          new Response("// stub", {
            status: 200,
            headers: { "Content-Type": "application/javascript" },
          });

        const caughtErrors: string[] = [];
        let respondWithCalled = false;
        let responseDelivered = false;

        try {
          const request = new Request(
            "chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/test.js",
          );
          let respondPromise: Promise<Response> | null = null;
          const event = {
            request,
            respondWith: (p: Response | Promise<Response>) => {
              respondWithCalled = true;
              respondPromise = Promise.resolve(p);
            },
          };

          try {
            fetchHandler(event);
          } catch (err) {
            caughtErrors.push(`fetchHandler threw: ${String(err)}`);
          }

          if (respondPromise) {
            try {
              await respondPromise;
              responseDelivered = true;
            } catch (err) {
              caughtErrors.push(`respondWith promise rejected: ${String(err)}`);
            }
          }

          // Let the microtask queue drain so the unawaited cache.put
          // rejection surfaces as unhandledrejection.
          await new Promise((r) => setTimeout(r, 200));
        } finally {
          window.fetch = realFetch;
          window.removeEventListener("unhandledrejection", onUnhandled);
          // Clean up any cache the SW source may have opened.
          try {
            const keys = await caches.keys();
            await Promise.all(
              keys
                .filter((k) => k.startsWith("bevy-naadf-"))
                .map((k) => caches.delete(k)),
            );
          } catch {
            // best-effort cleanup
          }
        }

        return {
          respondWithCalled,
          responseDelivered,
          unhandledRejections,
          caughtErrors,
        };
      },
      swSource,
    );

    // Post-fix: the SW must skip non-http(s) requests entirely. No
    // respondWith call, no cache.put attempt, no unhandled rejection.
    expect(
      result.respondWithCalled,
      `SW should not call respondWith for chrome-extension URLs; got: ${JSON.stringify(
        result,
        null,
        2,
      )}`,
    ).toBe(false);
    expect(result.unhandledRejections).toHaveLength(0);
    expect(result.caughtErrors).toHaveLength(0);
  });

  test("cacheFirst handles http(s) urls without error (control)", async ({
    page,
  }) => {
    const swSource = await fs.readFile(SW_PATH, "utf-8");
    await page.goto("/", { waitUntil: "commit" });

    const result = await page.evaluate(
      async (src): Promise<ReproResult> => {
        let fetchHandler: ((event: unknown) => void) | null = null;
        const sandboxSelf = {
          addEventListener: (type: string, handler: (event: unknown) => void) => {
            if (type === "fetch") fetchHandler = handler;
          },
          skipWaiting: () => {},
          clients: { claim: () => Promise.resolve() },
        };
        new Function("self", src)(sandboxSelf);

        const unhandledRejections: string[] = [];
        const onUnhandled = (e: PromiseRejectionEvent) => {
          unhandledRejections.push(String(e.reason));
          e.preventDefault();
        };
        window.addEventListener("unhandledrejection", onUnhandled);

        const realFetch = window.fetch;
        window.fetch = async () =>
          new Response("// stub", {
            status: 200,
            headers: { "Content-Type": "application/javascript" },
          });

        const caughtErrors: string[] = [];
        let respondWithCalled = false;
        let responseDelivered = false;

        try {
          const request = new Request(
            `${location.origin}/fake-asset-${Date.now()}.js`,
          );
          let respondPromise: Promise<Response> | null = null;
          const event = {
            request,
            respondWith: (p: Response | Promise<Response>) => {
              respondWithCalled = true;
              respondPromise = Promise.resolve(p);
            },
          };
          fetchHandler!(event);
          if (respondPromise) {
            await respondPromise;
            responseDelivered = true;
          }
          await new Promise((r) => setTimeout(r, 200));
        } catch (err) {
          caughtErrors.push(String(err));
        } finally {
          window.fetch = realFetch;
          window.removeEventListener("unhandledrejection", onUnhandled);
          try {
            const keys = await caches.keys();
            await Promise.all(
              keys
                .filter((k) => k.startsWith("bevy-naadf-"))
                .map((k) => caches.delete(k)),
            );
          } catch {
            // best-effort cleanup
          }
        }

        return {
          respondWithCalled,
          responseDelivered,
          unhandledRejections,
          caughtErrors,
        };
      },
      swSource,
    );

    expect(result.respondWithCalled).toBe(true);
    expect(result.responseDelivered).toBe(true);
    expect(result.unhandledRejections).toHaveLength(0);
    expect(result.caughtErrors).toHaveLength(0);
  });
});
