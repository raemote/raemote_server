# Troubleshooting

Start here — it checks the common causes and tells you what to do:

```sh
raemote doctor
```

Every check prints `ok`, `warn`, or `FAIL` with a hint. Add `--json` for scripts
and `--verbose` for relays and sockets.

If you need to go further, the daemon keeps a log:

```sh
raemote logs -n 50      # recent activity
raemote logs --follow   # stream it live
```

## The phone can't connect

1. On the computer: `raemote status` — is the daemon running with the expected
   paired/connected counts? If not, `raemote start`.
2. In the app: background and reopen it, or tap the refresh/reconnect control.
3. Make sure the phone has internet. The computer must be awake and online.
4. If `doctor` says the relay is unreachable, see *Restrictive networks* below.
5. If the app says it is not authorized, your device may have been revoked —
   pair again with `raemote pair`.

## No apps appear

- The app must answer an HTTP request at `/` like a web page. If it only serves
  an API or a non-HTTP protocol, it won't be listed.
- Confirm it is reachable on the computer: open `http://127.0.0.1:<port>` there.
- Run `raemote discover`, then `raemote apps list`.
- On macOS, only apps owned by your user are visible (system services stay
  private). Docker-published apps are user-owned and *are* visible.
- Still missing? Add it by hand: `raemote apps add myserver 8080`.

## Something is listed that isn't a web app

Local proxies (for example xray/sing-box) are excluded automatically. If
something else slips through:

```sh
raemote apps hide 127.0.0.1:10808     # or: raemote apps hide <name>
raemote apps unhide 127.0.0.1:10808
```

## The pairing link stopped working

Pairing links are intentionally short-lived. Mint a new one:

```sh
raemote pair
```

If you want a link that stays valid across restarts, see "Long-lived pairing
links" in the [README](../README.md).

## I can't tell my devices apart

Unnamed devices show as `device-<hex>`, so give them names:

```sh
raemote devices list
raemote devices rename <node-id> "Leo's iPhone"
```

The phone can also name itself (**About → This device**), and it sends its name
whenever it pairs. Names are for your convenience only — they never affect who
can connect.

## Restrictive networks (relay unreachable)

Some networks block the infrastructure that Raemote uses. Configure an outbound
proxy (HTTP CONNECT; SOCKS URLs are accepted and treated as HTTP):

```sh
raemote config set network.proxy http://host:port
raemote restart
```

`doctor` will then check the proxy instead of probing the relay directly.

## The service doesn't start at login

```sh
raemote service install
raemote service status
```

On Linux, the installer runs `loginctl enable-linger` so the user service
survives logout; if that failed, run it manually.

## An app shows an error page

- **"Couldn't reach the app"** — the app stopped or moved. Start it on the
  computer, then refresh the app list.
- **"took too long to respond"** — the app is busy; try again.
- **"unknown app"** — the app is gone from the list; tap refresh in the app.

## HTTPS apps

Discovery and proxying are HTTP-only for now. An app that only speaks HTTPS is
not reachable through Raemote yet; add it manually only if it also serves HTTP.

## Getting help

Gather `raemote doctor --json` and a few lines from `raemote logs`, then open an
issue: <https://github.com/raemote/raemote_server/issues>. For security problems,
follow [`SECURITY.md`](../SECURITY.md) instead of opening a public issue.
