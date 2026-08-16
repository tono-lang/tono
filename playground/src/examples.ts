/* Bundled snippets for the example picker. The first two mirror files under
   tono/examples; keep them small and self-contained (a playground document is
   one module, so no cross-module imports here). */

const PAYMENTS = `// Shared types other modules import. \`card\` and \`bank_account\` are private:
// visible only within this module, they are folded into the public
// \`payment_method\` union but are never referenced across a module boundary.
pub enum status { active, settled, refunded }

struct card { last4: string }

struct bank_account { iban: string }

@discriminator("kind")
pub union payment_method { card(card), bank(bank_account) }
`;

const HTTP_CLIENT = `@doc("A tiny API client: the token and endpoint resolve at construction.")
pub struct account {
  id: uuid
  email: string
}

pub struct client {
  api_token: string @env("API_TOKEN")
  endpoint: string @env("API_ENDPOINT") @default("https://api.example.com")

  op get_account(): account
    @http(method: "GET", path: "/account", endpoint: .endpoint)
}
`;

const BESPOKE_AUTH = `@doc("A tiny API whose auth is bespoke: the bearer header is built entirely from declared fields.")
pub struct account {
  id: uuid
  email: string
}

@doc("The SDK entry: the token and endpoint resolve at construction, the header derives from the token.")
pub struct client {
  api_token: string @env("API_TOKEN") @length(min: 1)
  auth_header: string @format("Bearer {.api_token}")
  endpoint: string @env("AUTH_ENDPOINT") @default("https://api.example.com")

  op get_account(): account
    @http(method: "GET", path: "/account", endpoint: .endpoint)
    @header("Authorization", .auth_header)
}

// Auth is 100% bespoke (there is no built-in scheme), but this scheme needs
// no bespoke code at all: @format derives the header from the declared
// token, and @header attaches it to every request.
`;

export interface Example {
  name: string;
  source: string;
}

export const EXAMPLES: Example[] = [
  { name: "Payment methods", source: PAYMENTS },
  { name: "HTTP client", source: HTTP_CLIENT },
  { name: "Bespoke auth", source: BESPOKE_AUTH },
];

export const DEFAULT_EXAMPLE = EXAMPLES[0];
