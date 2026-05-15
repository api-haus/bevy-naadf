// r2-proxy — serves objects from the bevy-naadf R2 bucket with the CORS /
// cross-origin-resource-policy headers the cross-origin-isolated web build
// needs (its page sends COOP/COEP — see `crates/bevy_naadf/_headers`).
//
// The deploy CI uploads `bevy-naadf.wasm` here; the production `init.js`
// fetches it from `https://bevy-naadf-assets.<account>.workers.dev/bevy-naadf.wasm`.
export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    const key = url.pathname.slice(1); // Remove leading slash

    if (!key) {
      return new Response("Not found", { status: 404 });
    }

    const object = await env.ASSETS.get(key);

    if (!object) {
      return new Response("Not found", { status: 404 });
    }

    const headers = new Headers();
    object.writeHttpMetadata(headers);
    headers.set("etag", object.httpEtag);

    // Required so the cross-origin-isolated page (COEP: require-corp) is
    // allowed to load this cross-origin response.
    headers.set("Cross-Origin-Resource-Policy", "cross-origin");
    headers.set("Access-Control-Allow-Origin", "*");

    // Cache for 1 year — the CI key-names uploads per commit, so they are
    // effectively immutable.
    headers.set("Cache-Control", "public, max-age=31536000, immutable");

    return new Response(object.body, { headers });
  },
};
