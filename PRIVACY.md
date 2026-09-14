# Privacy Policy

Raemote is built to keep your data on your own devices. This policy describes
what the Raemote Connector app (the **app**) and the Raemote server (the
**server**) do with your information.

## The short version

- No accounts, no sign-in, no advertising, no third-party analytics.
- We do not collect, transmit, or sell your personal data.
- Your apps, your pairing, and your settings stay on your own devices.

## What is stored, and where

**On your computer (the server)** — under `~/.raemote/`:

- a private key that identifies the server (`secret.key`);
- the identities of the devices you paired (`authorized_nodes`);
- your configuration (`config.toml`) and rotating logs.

**On your phone (the app):**

- the identities of the servers you paired with;
- web data (cookies, local storage) for apps you open, stored by iOS in the
  app's own container so your logins survive.

None of this is sent to us.

## What goes over the network

- Traffic between the app and the server is end-to-end encrypted.
- When a direct connection isn't possible, a relay forwards the traffic. A relay
  can see connection metadata (endpoints and timing) but not the encrypted
  contents.
- Pairing uses a short-lived token; the server is identified by its public key.

## Third parties

- The app and server send nothing to analytics, advertising, or tracking
  services.
- Connectivity uses the iroh relay and discovery infrastructure, currently
  operated by [n0](https://www.iroh.computer/); it is subject to their terms and
  is not controlled by Raemote.

## Children and sensitive data

Raemote is a self-hosted tool: you choose what to expose through it. Do not use
it to expose data you are not allowed to access.

## Changes

This policy lives in the repository and will be updated there as the product
changes.

## Contact

Questions about privacy: **cool@lyuhj.top**
