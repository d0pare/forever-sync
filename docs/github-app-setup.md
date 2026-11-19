# Registering the GitHub App

Forever Sync signs users in through a **GitHub App** using the **device flow**. The app is registered under a personal account for now. There is no backend, so the app needs **no client secret, no private key and no webhook**. The desktop app embeds only the public **Client ID** and **slug**.

## 1. Create the app

Open **https://github.com/settings/apps/new** (Settings → Developer settings → GitHub Apps → New GitHub App) and fill in:

| Field | Value |
|---|---|
| **GitHub App name** | `Forever Sync` (see "Naming" below) |
| **Description** | `Syncs your World of Warcraft: Forever settings across your computers through a private GitHub repository you own. Not affiliated with Blizzard Entertainment.` |
| **Homepage URL** | `https://github.com/d0pare/forever-sync`, the project's source repo. Required, but GitHub doesn't check that it exists, and it can be changed any time. While the app is installable only on your account, nobody else sees it. Make the repo public, or point this at a website, before opening the app to others. |

**Identifying and authorizing users**
| Setting | Value | Why |
|---|---|---|
| Callback URL | *leave empty* | Device flow needs no redirect |
| Expire user authorization tokens | ✅ **checked** | 8-hour tokens plus a 6-month refresh token. Device-flow tokens refresh without a client secret. |
| Request user authorization (OAuth) during installation | ☐ unchecked | We authorize through device flow, not during install |
| **Enable Device Flow** | ✅ **checked** | Required |

**Post installation**: leave Setup URL empty and *Redirect on update* unchecked.

**Webhook**: **uncheck "Active"**. We have no server to receive webhooks.

**Permissions → Repository permissions**
| Permission | Access |
|---|---|
| **Contents** | **Read and write** (clone, fetch, push) |
| Metadata | Read-only (mandatory, preselected) |
| *everything else* | No access |

**Account permissions**: none. **Organization permissions**: none.

> **No Administration permission.** It would let the app create repos through the API, but it also grants delete and settings rights on every repo the app is installed on. Instead, onboarding has the user create the repo from a prefilled link and install the app on that one repo (see DESIGN.md §8.2).

**Where can this GitHub App be installed?** → **Only on this account** for now. Switch to *Any account* (Advanced → Make public) before anyone else uses Forever Sync. This only changes visibility; the settings above stay the same.

Click **Create GitHub App**.

## 2. Record the public identifiers

On the app's settings page, note:
- **Client ID** (`Iv23li…`): the desktop app uses it for device flow.
- **App ID** (a number): for reference.
- **Slug**: the last part of the app's public URL, `https://github.com/apps/<slug>`. It's used for the install link `https://github.com/apps/<slug>/installations/new`.

These are public and safe to commit. They will live in `crates/foreversync-github/src/config.rs`.

**Do not** click *Generate a new client secret* or *Generate a private key*. Neither is needed. If you already created one, delete it on the same page.

**Optional:** upload a logo under *Display information*. Use original artwork only, with no Blizzard or WoW assets.

## 3. Smoke-test the device flow (about 2 minutes)

Run this in a terminal. Tokens appear only in your terminal; don't paste them anywhere.

```sh
CLIENT_ID=Iv23li...   # your Client ID

# 1) ask for a device code
curl -s -X POST https://github.com/login/device/code \
  -H "Accept: application/json" -d "client_id=$CLIENT_ID"
# → {"device_code":"…","user_code":"ABCD-1234","verification_uri":"https://github.com/login/device",…}

# 2) open https://github.com/login/device, enter the user_code, authorize

# 3) exchange the device code for tokens
DEVICE_CODE=...       # from step 1
curl -s -X POST https://github.com/login/oauth/access_token \
  -H "Accept: application/json" \
  -d "client_id=$CLIENT_ID&device_code=$DEVICE_CODE&grant_type=urn:ietf:params:oauth:grant-type:device_code"
# → {"access_token":"ghu_…","expires_in":28800,"refresh_token":"ghr_…","refresh_token_expires_in":15897600,…}
```

Then check repo access end to end:

1. Create a private test repo, e.g. https://github.com/new?name=forever-sync-test&visibility=private
2. Install the app on **only** that repo: `https://github.com/apps/<slug>/installations/new` → *Only select repositories*.
3. Check:
   ```sh
   TOKEN=ghu_...
   curl -s -H "Authorization: Bearer $TOKEN" https://api.github.com/user/installations
   curl -s -H "Authorization: Bearer $TOKEN" https://api.github.com/repos/<you>/forever-sync-test
   ```
   The repo should be listed, with `"private": true` and `"permissions": {"push": true, …}`.
4. Test the refresh **without a client secret** (this closes spike S1's refresh question):
   ```sh
   curl -s -X POST https://github.com/login/oauth/access_token \
     -H "Accept: application/json" \
     -d "client_id=$CLIENT_ID&grant_type=refresh_token&refresh_token=ghr_..."
   ```
   This should return a new `ghu_…` / `ghr_…` pair. The old pair stops working.

Afterwards you can delete the test repo, or keep it for development.

## Naming

**Decided: "Forever Sync"** (slug `forever-sync`, install link `https://github.com/apps/forever-sync/installations/new`). On 2026-11-18 no public GitHub App or account used this slug. Private apps can't be checked, so GitHub confirms availability when you register. If the name is taken, fall back to "Forever Sync App" (`forever-sync-app`).

Rules we follow:
- Don't use "Wow Sync". It is already a public GitHub App (`github.com/apps/wow-sync`) for another WoW addon-sync project.
- Keep "World of Warcraft", "Warcraft" and "Blizzard" out of the name, and use no Blizzard artwork. Mentioning "World of Warcraft: Forever" in the description, together with the non-affiliation line, is fine.
- The desktop app, the GitHub App and the data repo (`forever-sync-data`) all use the same name, so users recognize what they're authorizing.
