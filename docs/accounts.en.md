# Microsoft account setup

> **Preview status:** Microsoft sign-in is implemented but has not been verified with a real account. A CCL OAuth application has not yet been registered. The complete Microsoft, Xbox, and Minecraft flow must pass with that application before this feature can meet the CLI preview acceptance gate.

## Register a public desktop application

The launcher uses Microsoft's device authorization flow. It is a public desktop client and must not use a client secret. Microsoft documents desktop app registration and public-client configuration in the [desktop app registration guide](https://learn.microsoft.com/en-us/entra/identity-platform/scenario-desktop-app-registration) and its [device code flow reference](https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-device-code).

1. Sign in to the [Microsoft Entra admin center](https://entra.microsoft.com/) with an account allowed to create app registrations.
2. Open **Identity → Applications → App registrations → New registration**.
3. Give the application a clear launcher name. For the account type, select **Personal Microsoft accounts only** for the currently implemented `/consumers` authority. Microsoft personal accounts are the intended account type for a Minecraft Java owner. If the portal does not offer this choice, stop and confirm the registration design before using a broader type.
4. Register the application and copy its **Application (client) ID**. This GUID is public configuration, not a password.
5. Open **Authentication**. Under **Advanced settings**, set **Allow public client flows** to **Yes**, then save. Device-code clients do not use a client secret or a browser redirect URI.
6. Do not create a client secret, and do not put access or refresh tokens in launcher settings.
7. Confirm the current Xbox and Minecraft service access requirements for your application. Registration alone does not prove that the complete sign-in flow will work. If a service rejects the application, resolve access using the returned error and that service’s official support process.

Microsoft's [app registration quickstart](https://learn.microsoft.com/en-us/entra/identity-platform/quickstart-register-app) describes supported account types. The desktop guide explains when a desktop client is a public client and how to enable public-client flows. This project has not yet validated whether its application registration and API access are approved for public users.

## Configure and sign in

Save the client ID in the launcher's user configuration:

```sh
ccl settings set auth.microsoft_client_id YOUR-APPLICATION-CLIENT-ID
ccl account check-storage
ccl account login
```

`account login` tests the credential vault before contacting Microsoft and stops before requesting a device code if the vault cannot safely store credentials. `account check-storage` runs the same write/read/delete probe on demand; it uses a temporary sentinel and does not alter saved accounts.

Or pass an ID for one login:

```sh
ccl account login --client-id YOUR-APPLICATION-CLIENT-ID
```

The launcher prints a Microsoft verification address and one-time device code. Open that address in a browser, sign in, and enter the code. Do not share the code; anyone who enters it first could authorize the pending login. The CLI waits for completion. Then use:

```sh
ccl account list
ccl account select ACCOUNT-ID
ccl launch VERSION
```

With a selected account, `launch` uses it by default and refreshes credentials when needed. To launch offline instead, pass `--offline` or `--username NAME`. `--account ACCOUNT-ID` selects a specific saved account for one launch. `account logout [ACCOUNT-ID]` removes local credentials and account metadata; it does not revoke Microsoft consent.

## Credential storage

Access and refresh tokens are stored only through the operating system credential store. Account names and IDs are kept separately in `accounts.json`; that metadata file contains no tokens. On macOS and Windows the operating system credential vault is used. On Linux, a Secret Service implementation reachable through the active D-Bus session must be installed and unlocked. A headless system without a persistent Secret Service cannot use Microsoft login. There is no plaintext credential fallback. A graphical desktop is not required if you provide and unlock a persistent Secret Service in your session.

If secure storage is unavailable, login or refresh fails with an error. Start a desktop session with an unlocked keyring, or use offline launch until secure storage is available. Do not work around this by copying tokens into a file or shell command.

## Validation checklist for the application owner

Before calling account sign-in ready for a public preview, record evidence that:

- the registered public client ID is the one shipped/configured for the launcher and has completed required Microsoft review;
- device-code login works for a Minecraft Java account with an entitlement;
- denial, expiry, cancellation, refresh, sign-out, and unavailable credential-vault paths behave correctly;
- no tokens appear in CLI output, logs, error text, ordinary configuration, or account metadata;
- the Linux build works with a real persistent Secret Service/D-Bus session;
- the service/API permissions and distribution comply with current Microsoft, Xbox, and Minecraft requirements.

Until those checks are complete, the authentication feature and preview package must be described as **unverified and not ready for public distribution**.
