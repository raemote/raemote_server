<p align="center">
  <img src="Raemote_ICON.png" width="112" alt="Raemote icon">
</p>

# Raemote

Use the web apps running on your computer — from your phone, anywhere.

Raemote is a small server you run on your computer. It finds the web apps you
already have open locally (Jellyfin, Home Assistant, a dev server, anything on
`http://localhost`) and lets you reach them from the Raemote iOS app.

## Why it exists

Lots of useful things only run on your own computer. To use them from your
phone you normally need a VPN, a public IP, or port forwarding — all fiddly and
easy to get wrong.

Raemote pairs your phone with your computer **once**, then handles discovery and
connection for you. No accounts, no cloud, no router setup.

- **Simple** — install with one command, scan one QR code.
- **Private** — the connection is end-to-end encrypted. It goes directly
  between your phone and computer when possible; if not, a public relay
  forwards the (still encrypted) traffic but can't read it.
- **Automatic** — Raemote scans your computer for local web apps so you don't
  have to configure each one.

## Requirements

- macOS (Apple Silicon and Intel) or Linux (x86_64 and arm64, e.g. a Raspberry Pi).
- The Raemote iOS app on your phone.
- Both the computer and the phone need internet access.

## Installation

One command:

```sh
curl -fsSL https://github.com/raemote/raemote_server/raw/main/install.sh | sh
```

This downloads the latest release, installs the `raemote` command into
`~/.local/bin`, and sets up a background service so it keeps running and starts
again when you log in.

On networks where GitHub is slow or unreliable, use the Gitee mirror instead —
the installer then prefers the Gitee release rather than reaching for GitHub:

```sh
curl -fsSL https://gitee.com/pppkin/raemote_server/raw/main/install.sh | sh
```

Prefer to inspect first? It's fine to download and read the script before
running it:

```sh
curl -fsSL https://github.com/raemote/raemote_server/raw/main/install.sh -o install.sh
less install.sh
sh install.sh
```

### Common options

```sh
# Install without starting the background service
curl -fsSL https://github.com/raemote/raemote_server/raw/main/install.sh | sh -s -- --no-service

# Install somewhere else
curl -fsSL https://github.com/raemote/raemote_server/raw/main/install.sh | sh -s -- --bin-dir ~/bin
```

The installer adds `~/.local/bin` to your shell profile (`~/.zshrc` and
`~/.bashrc`) when it isn't already there. Reload your shell — or open a new
terminal — to run `raemote` by name; until then, use the full path.

## Pairing

Pairing links your phone to this computer. You only do it once.

1. On the computer, show the pairing code:

   ```sh
   raemote pair
   ```

   This prints a `raemote://...` link and a QR code.

2. On your phone, open the Raemote app, tap **+**, and either **Scan QR Code**
   or choose **Manual Setup** and paste the link.

That's it — the app now shows the web apps found on your computer. Tap one to
open it.

> If your phone says it isn't connected later, open the app and tap the refresh
> button; pair again only if it asks you to.

### Inviting another device

Already paired? You can introduce a second phone/tablet without going back to
the computer: on the paired device open the server, tap **… → Invite Device…**,
and let the other device scan the code it shows.

The invitation is **one-time** and expires after a few minutes; whoever redeems
it first becomes a paired device. You can disable this on the server with
`allow_invites = false` under `[bind]`.

## Everyday use

```sh
raemote status          # is it running, and how many apps were found?
raemote pair            # show the pairing link + QR code
raemote devices list    # list paired devices (and their names)
raemote devices rename <node-id> "Leo's iPhone"
raemote devices revoke <node-id>
raemote discover        # rescan for local web apps now
raemote apps list       # list the apps Raemote found
raemote stop            # stop it (starts again at your next login)
raemote start           # start again
```

Found the wrong apps, or want to add one manually? Add or hide apps without
editing files:

```sh
raemote apps add myserver 8080          # pin an app discovery missed
raemote apps hide 127.0.0.1:10808       # hide a false positive (or use its name)
raemote apps unhide 127.0.0.1:10808
```

Discovered apps are automatic and temporary. Manual `[[apps]]` entries are
permanent. For anything else, edit the config with `raemote config edit`.

## Naming

The server tells your phone which computer it is. By default that is the machine
hostname; set your own with:

```sh
raemote config set name "Living Room Mac mini"
```

Paired devices have names too (see `raemote devices rename`), and in the app you
can give any server an **alias** (Server → Name) that overrides the name the
server reports.

## Long-lived pairing links

By default a pairing token is short-lived and is regenerated whenever the daemon
restarts. To keep a link valid for a long time, pin a fixed token and set a long
lifetime:

```toml
[bind]
token = "put-64-random-hex-characters-here"
token_ttl_secs = 3153600000   # ~100 years (larger values are clamped)
```

```sh
raemote pair
```

The link is then stable across restarts and effectively permanent. The token can
also be supplied via the `RAEMOTE_BIND_TOKEN` environment variable, which suits
service installs.

Treat this link as a long-lived secret: anyone who holds it can pair a device.
Remove `token` (or unset the variable) and restart when it is no longer needed.
To mint a long-lived link without editing the config, use
`raemote pair --ttl <seconds>`.

## Not finding an app?

Raemote lists things that answer an HTTP request at `/` like a web page.

- Make sure it's running and reachable at `http://127.0.0.1:<port>` on the computer.
- Run `raemote discover`, then check `raemote apps list` and `raemote doctor`.
- On macOS, Raemote can only see apps owned by your own user (a good thing —
  system services stay private).
- If it's still missing (for example an app that errors on `/`), add it by hand:

  ```sh
  raemote apps add myserver 8080
  ```

## Uninstall

```sh
curl -fsSL https://github.com/raemote/raemote_server/raw/main/install.sh | sh -s -- --uninstall
```

Add `--purge` to also delete Raemote's settings and identity in `~/.raemote`.

## Troubleshooting

Run the built-in health check first — it verifies the daemon, permissions,
config, discovery, and connectivity, and tells you what to do about anything it
finds:

```sh
raemote doctor
```

If something still isn't working, the daemon keeps a log:

```sh
raemote logs -n 50      # recent activity
raemote logs --follow   # stream it live
```

See [docs/troubleshooting.md](docs/troubleshooting.md) for common problems and
their fixes.

## Documentation

- [How it works](docs/how-it-works.md) — the model on one page.
- [Troubleshooting](docs/troubleshooting.md) — common problems and fixes.
- [Threat model](docs/threat-model.md) and [security policy](SECURITY.md).
- [Privacy policy](PRIVACY.md) and [acceptable use](docs/acceptable-use.md).
- [Installing from an AI agent](docs/agent-install.md).
- [Releasing](RELEASING.md) — cutting a release.

## Repository

- GitHub: <https://github.com/raemote/raemote_server>
- Gitee (mirror): <https://gitee.com/pppkin/raemote_server>

## License

Raemote is licensed under the **GNU Affero General Public License, version 3 or
later** — see [LICENSE](LICENSE). In short: you are free to use, study, and
modify it, but if you run a modified version as a network service, you must
offer the corresponding source to its users.

