# RFC 037: native web stdlib redesign

- **Status:** Planned
- **Created:** 2026-03-07
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 023 (Compilable stdlib and `rust.module` handoff)
    - RFC 024 (extensible derive protocol)
    - RFC 027 (Vocabulary/desugaring infrastructure)
    - RFC 031 (Library system)
    - RFC 033 (`ctx`, typed configuration and shared resources)
    - RFC 035 (First-class named function references)
    - RFC 036 (User-defined decorators)
    - RFC 066 (`std.http` HTTP client surface)
    - RFC 094 (context managers)
    - RFC 095 (`span` blocks and traces)
    - RFC 103 (`std.secrets`)
    - RFC 104 (capabilities, limits and receipts)
    - RFC 141 (`ArgumentsOf` and type patterns for handler extraction)
    - RFC 144 (type-owned constants)
    - RFC 147 (Result matching with `except`)
    - RFC 150 (deprecation diagnostics)
    - RFC 156 (typed MCP servers)
    - RFC 160 (vocabularies)
    - RFC 165 (`std.db`)
    - RFC 168 (`std.primitives`)
- **Issue:** [#171](https://github.com/encero-systems/incan/issues/171)
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

`std.web` is Incan's standard library for serving HTTP applications and APIs. A route is a typed function turned into a value by a decorator, and an application is built from the routes it lists. A handler's parameters say what it needs from a request, and that one idea carries validation, identity and dependency provision: asking for `member: Member` means the route cannot run without a member. Sessions, typed cookies, guards, error handling, cross-site protection and caching defaults are library code on ordinary language features. Nothing here needs the compiler to know about the web.

This RFC owns the foundation for serving over HTTP. A vocabulary or a higher-level framework can make it shorter, but must lower to the same model.

## Core model

1. **Routes are values.** `@web.get` and `@web.post` turn a function into a route value, and `web.App(routes=[...], groups=[...])` lists what an application serves. A module holds only declarations, so nothing registers as a side effect.
2. **A parameter says what a handler needs.** Any type adopting `FromRequest` can be built from a request. `Json[T]`, `Query[T]`, `Form[T]` and your own types all work the same way.
3. **Wrappers decide who handles failure.** With `T`, a failure means the route does not run. `Option[T]` gives `None` when the value is absent. `Result[T, E]` hands the failure to the route.
4. **Identity is a parameter.** A type adopting `Authenticated[C]` is built from a credential `C` by one `authenticate` function, so there is no separate auth setting to forget.
5. **A guard decides what the caller may do.** It is a function whose parameters are filled in like a handler's, and it runs before the route.
6. **Shared state is a `ctx`.** Stores and settings live in an RFC 033 context, so `std.web` adds no state mechanism.
7. **Cookies are declared.** A cookie is a model with rules and a stated purpose, and sessions are built on that layer.
8. **Errors become responses in one place.** `@web.on_error` turns an error type into a response for any caller.
9. **The safe behavior is the default.** Cross-site protection, private caching of identified responses, and hardened cookies need no code from the author.
10. **Capabilities are a separate question.** Auth asks what this caller may do, and RFC 104 asks what this program may do. Both must pass.

## Motivation

The current `std.web` proves that Incan can compile web programs, but it does not provide a native, coherent API platform experience.

Current problems:

- Users still encounter backend leakage such as explicit wrapper types and backend-oriented handoff details.
- Routing behavior is split across compiler logic, macros, and runtime helpers.
- The platform does not present one complete framework story for auth, middleware, validation, docs, and lifecycle.

The goal is not just "web works." The goal is that Incan developers can expose networked applications and APIs with one coherent server-side mental model, in which the compiler core provides general primitives and framework ownership lives in the library.

Authentication is where this matters most, because mistakes there are silent. A view that forgets a login decorator still runs for anyone, a member's page can be cached and served to the next visitor, and a cookie is sent with requests other sites trigger. This RFC makes those mistakes impossible to write, or refuses them when the application starts.

## Goals

- Provide a native `std.web` surface for serving HTTP applications and APIs, with typed handlers, request extraction, response conversion, middleware, lifecycle and docs metadata.
- Let an application have login, sessions, tokens, per-area permissions and cookies using only the standard library.
- Make the secure behavior the default, and refuse dangerous combinations when an application starts.
- Keep every mechanism ordinary library code on general language features.
- Keep backend framework details out of the public API.
- Stay compatible with RFC 066 where server and client concepts overlap.

## Non-Goals

- Defining the `std.http` client API, retry model, redirect behavior, timeout policy, or response decoding surface; those belong to RFC 066.
- Defining capabilities, limits or receipts; those belong to RFC 104, and this RFC uses them.
- Consent banners, recorded consent choices and generated cookie notices. A higher layer supplies these through `CookiePolicy` and `app.cookies()`.
- JWT validation, OAuth login, password reset and email verification flows, breached-password checks, login-attempt policy, and role or permission systems. These change more often than a compiler release, so they belong above the standard library, and they plug in through `Credential`, `Authenticated`, `FromRequest` and `Guard`.
- Sandboxing hostile code. Limits are a safety net for code that declares its capabilities.
- Exact grammar for every DSL form, and wire-level details for gRPC or Arrow-oriented transports.

## Guide-level explanation (how users think about it)

### Providing an API with `std.web`

The primary experience should feel close to FastAPI:

```incan
from std import web

@web.get("/")
async def index() -> dict[str, str]:
    return {"message": "Hello World"}

@web.get("/users/{id}")
async def get_user(id: int) -> User:
    return load_user(id)?

@web.post("/users")
async def create_user(body: CreateUser) -> User:
    return save_user(body)?

def main():
    web.App(routes=[index, get_user, create_user]).run(port=8080)
```

From a user's point of view:

- route decorators are real decorators that turn a function into a route value, not marker-only hacks
- an application is built from the routes it lists, so a module holds only declarations and nothing registers as a side effect
- handler signatures describe extraction behavior
- plain Incan values are returned, and the framework handles response conversion

A handler may be `def` or `async def`. A plain `def` runs off to the side, so slow code in it cannot hold up other requests.

### Building an application

A function builds the application, so settings can come from configuration and a test can build its own:

```incan
def build_app() -> web.App:
    return web.App(
        routes=[home, login_page, login, logout, members_page],
        groups=[
            web.Group("/admin", guards=[requires_role("admin")], routes=[stats]),
            web.Group("/account", guards=[only_own_account], routes=[devices, end_device]),
        ],
        on_error=[send_to_login, forbidden_page],
        sessions=AppState.sessions,
        public_origin=AppState.public_origin,
    )

def main() -> Result[None, ConfigError]:
    AppState.preflight()?
    build_app().run(port=AppState.port)
    return Ok(None)
```

### Asking for what a handler needs

| Parameter | What the handler gets |
| --- | --- |
| `T` | The value. If it cannot be built, the handler does not run. |
| `Option[T]` | `None` if it is absent. Anything else wrong still stops the request. |
| `Result[T, E]` | `Ok(value)` or `Err(problem)`, and the handler decides what to do. |

This lets a page show its own message for a bad form:

```incan
@web.post("/join")
async def join(form: Result[web.Form[SignupForm], web.FormError]) -> web.Response:
    match form:
        case Ok(values):
            return await finish_joining(values)
        case Err(problem):
            return web.Response.html(render_join_form(problem.fields), status=422)
```

Each type says what "absent" means. For a credential it is no credential, and a bad token is a failure. For a form it is an empty body, and a bad field is a failure.

Any type can adopt `FromRequest`, which is also how dependencies are provided. A `Transaction` type that opens a database transaction for the request is a parameter like any other, and each type is built once per request, so a guard and a route that both ask for `Member` share one lookup.

### Sharing stores and settings

Everything an application shares is an RFC 033 `ctx`, which is built on first read, is read-only, is safe from any task, and is overridden per test with `ctx_override`:

```incan
def open_members(members_path: str) -> Result[MemberStore, StoreError]:
    return MemberStore.open(members_path)

def open_sessions(sessions_path: str) -> Result[web.auth.DbSessions, StoreError]:
    return web.auth.DbSessions.open(sessions_path, expire_after=TimeDelta.hours(12), idle_after=TimeDelta.hours(2))

ctx AppState(env_prefix="APP_"):
    port: int = 8080
    public_origin: str = "http://localhost:8080"
    members_path: str = "members.db"
    sessions_path: str = "sessions.db"
    members [default_factory=open_members]: MemberStore
    sessions [default_factory=open_sessions]: web.auth.DbSessions
```

### Who is calling?

A route that needs to know the caller asks for the caller:

```incan
@web.get("/me")
async def me(member: Member) -> web.Json[Member]:
    return web.Json(member)
```

`Member` is your own model. It says how to work out who is calling:

```incan
from std.web.auth import Authenticated, Bearer, Unauthorized

model Member with Authenticated[Bearer]:
    id: int
    name: str
    roles: list[str]

    @staticmethod
    async def authenticate(credential: Bearer) -> Result[Member, Unauthorized]:
        return await AppState.members.find_by_token(credential.token)
```

Every authentication method answers two separate questions: how does the proof travel, and how is it checked? A `Credential` answers the first, and `authenticate` answers the second.

| Credential | How the proof travels |
| --- | --- |
| `Bearer` | The `Authorization: Bearer ...` header |
| `ApiKey` | Its own header |
| `SessionCookie[T]` | A session cookie |

A stored token and a JWT travel the same way, so the difference is one line in the body of `authenticate`. Signing in through another provider ends by starting a session, so later requests are an ordinary `SessionCookie`.

### Accepting two methods on one route

An enum with `@derive(Credential)` tries its variants from top to bottom, and the first one present is used:

```incan
@derive(Credential)
enum Login:
    Session(SessionCookie[MemberSession])
    Token(Bearer)

model Member with Authenticated[Login]:
    id: int
    name: str
    roles: list[str]

    @staticmethod
    async def authenticate(credential: Login) -> Result[Member, Unauthorized | StoreError]:
        match credential:
            Login.Session(session) => return await AppState.members.find(session.data.member_id)
            Login.Token(bearer) => return await AppState.members.find_by_token(bearer.token)
```

Routes do not change. Because `match` must cover every variant, adding a method later does not compile until it is handled.

### What failure means

| `authenticate` returns | With a session cookie | With any other credential |
| --- | --- | --- |
| `Unauthorized` | The session is ended and the cookie cleared, and the request then counts as having no credential. | The request stops with a 401. |
| Any other error | `on_error` handles it, a 503 by default, and the session is kept. | The same. |

So a removed member becomes a visitor, a script with a bad token is told, and an outage never logs anyone out. A credential that is present but wrong stops the request and does not fall through to the next variant. An expired or unknown session cookie counts as no cookie.

### Logging in and out

The server stores a small record under a long random id, and the browser keeps only that id in a cookie:

```incan
from std.web.auth import SessionData

@derive(json)
model LoginForm:
    email: str
    password: SecretStr
    remember: bool = false

@derive(json)
model MemberSession with SessionData:
    member_id: int
    device: str

    property owner -> Option[str]:
        return Some(str(self.member_id))

@web.post("/login")
async def login(form: web.Form[LoginForm], back: web.ReturnTo, request: web.Request) -> web.Response:
    match await AppState.members.check_password(form.email, form.password):
        case Ok(member):
            session = MemberSession(member_id=member.id, device=request.user_agent())
            response = web.Response.redirect(back.path_or("/members"))
            if form.remember:
                return response.start_session(session, remember_for=TimeDelta.days(30))
            return response.start_session(session)
        case Err(_):
            return web.Response.html(render_login_form("Wrong email or password"), status=401)

@web.post("/logout")
async def logout() -> web.Response:
    return web.Response.redirect("/").end_session()
```

The session model states its owner, so "log out everywhere" cannot miss a session. A model returns `None` for a session with no owner, such as a visitor's cart. A session started as one model cannot be read as another, so a half-logged-in `PendingSession` is never a `MemberSession`, and two-factor login swaps one for the other with `start_session`.

The cookie goes away when the browser closes, unless the person asked to stay logged in. Consent rules commonly exempt a login cookie that ends with the browser session, and treat a "keep me logged in" tick box as the way to ask for a longer one.

A members page can list and end devices. A `handle` is a separate value for a session that is safe to show, and nobody can log in with it:

```incan
@web.get("/devices")
async def devices(member: Member, current: SessionCookie[MemberSession]) -> web.Html:
    sessions = await AppState.sessions.for_owner(str(member.id))
    return render_devices(sessions, current.handle)

@web.post("/devices/{handle}/end")
async def end_device(handle: str, member: Member) -> web.Response:
    await AppState.sessions.end(handle, owner=str(member.id))
    return web.Response.redirect("/account/devices")
```

### Passwords

`std.password` provides password hashing, and is separate from `std.hash` because the two have opposite goals. It needs no web server, and a login form needs nothing beyond the standard library:

```incan
from std.password import PasswordCheck, hash_password, verify_password

class MemberStore:
    async def create(self, email: str, password: SecretStr) -> Result[MemberRecord, StoreError]:
        digest = await hash_password(password)
        return await self.insert(email, digest)

    async def check_password(self, email: str, password: SecretStr) -> Result[MemberRecord, Unauthorized]:
        record = await self.find_by_email(email)

        match await verify_password(password, record, (r) => r.password_hash):
            case PasswordCheck.Valid(member):
                return Ok(member)
            case PasswordCheck.Upgrade(member, new_hash):
                await self.set_password_hash(member.id, new_hash)
                return Ok(member)
            case PasswordCheck.Invalid:
                return Err(Unauthorized("Wrong email or password"))
```

`record` is an `Option`, so an unknown email spends the same time hashing and returns `Invalid`, and response time cannot reveal who has an account. `Upgrade` replaces an old hash at the next successful login, so settings can get stronger without forcing a reset.

### Typed cookies

A cookie is a model that says what it holds and what its rules are:

```incan
from std.web.cookies import Cookie, CookieRules, Purpose

@derive(json)
model Theme with Cookie:
    dark: bool

    pub const RULES: CookieRules = CookieRules(
        name="theme",
        purpose=Purpose.Preference,
        lasts=TimeDelta.days(365),
    )

@web.get("/")
async def home(theme: Option[Theme]) -> web.Html:
    return render_home(theme)

@web.post("/theme")
async def choose_theme(form: web.Form[ThemeForm]) -> web.Response:
    return web.Response.redirect("/").set_cookie(Theme(dark=form.dark))

@web.post("/theme/reset")
async def reset_theme() -> web.Response:
    return web.Response.redirect("/").clear_cookie[Theme]()
```

`purpose` is required, because consent rules depend on it: `Necessary`, `Preference`, `Analytics` or `Marketing`. Leaving out `lasts` means the cookie goes away when the browser closes. Session ids and the remembered return path are ordinary cookies on this same layer.

`std.web` does not decide anything about consent. `app.cookies()` lists every declared cookie with its purpose and lifetime, and a `CookiePolicy` can hold back any cookie that is not `Necessary`:

```incan
trait CookiePolicy:
    def allows(self, rules: CookieRules, request: web.Request) -> bool

def build_app() -> web.App:
    return web.App(routes=[home, choose_theme], cookie_policy=MyPolicy())
```

### Turning errors into responses

`@web.on_error` decides the response for an error type, wherever it came from. This one covers sending browsers to log in and telling scripts plainly:

```incan
@web.on_error
def send_to_login(error: Unauthorized, request: web.Request) -> web.Response:
    if request.accepts_html():
        return web.Response.redirect("/login").return_to(request.full_path)
    return error.to_response()
```

`.return_to` remembers the path in a short-lived cookie, and `web.ReturnTo` reads it back. This avoids `/login?next=...`, a known attack in which a real login link sends you to a fake site afterward. A custom 404 page, a `Forbidden` page and a JSON error format use the same mechanism.

### Are they allowed?

A guard runs before the route and says yes or no. It can sit on the app, a group or a route, and all that apply must pass:

```incan
from std.web import Forbidden, Guard, guard

def requires_role(role: str) -> Guard:
    @guard
    def check(member: Member) -> Result[None, Forbidden]:
        if role in member.roles:
            return Ok(None)
        return Err(Forbidden(f"Requires the {role} role"))
    return check

@guard
async def owns_post(id: int, member: Member) -> Result[None, Forbidden]:
    post = await AppState.posts.get(id)
    if post.author_id == member.id:
        return Ok(None)
    return Err(Forbidden("This is not your post"))

@web.delete("/posts/{id}", guards=[owns_post])
async def delete_post(id: int) -> web.Response:
    await AppState.posts.remove(id)
    return web.Response.redirect("/members")
```

A guard may instead return an RFC 104 `Restriction`, which limits what the rest of the request can do. If a route forgets an ownership check, the operation itself refuses:

```incan
@guard
def only_own_account(member: Member) -> Restriction:
    return Restriction(manage_members, member_id=member.id)

def account_group() -> web.Group:
    return web.Group("/account", guards=[only_own_account], routes=[delete_account])
```

`std.web` has no idea what a role is. `member.roles` above is your own field.

### Declaring what a route needs

`caps=` on a route or group states what it may use, under RFC 104. It is optional, and when present it must be true:

```incan
@web.get("/devices")                            # not checked, still leaves receipts
async def devices(member: Member) -> web.Html:
    ...

@web.post("/delete", caps=[manage_members])     # checked, and a ceiling while it runs
async def delete_account(member: Member) -> Result[web.Response, StoreError | CapabilityDenied]:
    await AppState.members.remove(member.id)?
    return Ok(web.Response.redirect("/").end_all_sessions(owner=str(member.id)))
```

A refusal is an ordinary `CapabilityDenied` error. `?` passes it to `on_error`, a 403 by default, and RFC 147's `except` handles it where a route wants its own message.

### Protection from other sites

A browser attaches your cookie to any request to your site, even when another site's page triggers it, so a hidden form could act as a logged-in member. `App` checks where a request came from, on by default, using the `Sec-Fetch-Site` header browsers send and page scripts cannot fake:

| What a `POST`, `PUT`, `PATCH` or `DELETE` shows | Result |
| --- | --- |
| `same-origin`, or `none` (typed into the address bar) | Allowed |
| `cross-site` or `same-site` | Refused with `CrossSiteRefused`, a 403 by default |
| No such header, but an `Origin` header | Allowed only if `Origin` matches |
| Neither header | Allowed, because it is not a browser, so there is no attached login to abuse |

No forms carry a token. For the cases that need something different:

```incan
# A login provider that posts back from its own site
@web.post("/login/callback", cross_site=True)
async def callback(form: web.Form[ProviderReply]) -> web.Response:
    return await finish_login(form)

def build_app() -> web.App:
    return web.App(
        routes=[callback],
        # A token API called from browser code on other sites
        groups=[web.Group("/api", cross_site=True, routes=[star_preview])],
        trusted_origins=["https://docs.example.com"],
        public_origin="https://example.com",
    )
```

A route that accepts a session cookie cannot be `cross_site=True`. That combination reopens the hole, so the application refuses to start. A token API gets its own identity type, such as `ApiCaller with Authenticated[Bearer]`.

### Caching

A CDN saves responses and serves them to the next visitor, so one member's page must never be saved. `std.web` knows what each request read, and sets the caching header itself:

| The response | Caching header |
| --- | --- |
| Read a credential, even `Option[Member]` with nobody logged in | `private, no-store`, and this cannot be overridden |
| Sets or clears a cookie, including `start_session` | `private, no-store`, and this cannot be overridden |
| Read a typed cookie like `Theme` | `private`, unless the route says otherwise |
| Read none of these | No header, so the route chooses |

```incan
@web.get("/")
@web.cache(max_age=TimeDelta.minutes(10))
async def home() -> web.Html:
    return render_home()
```

`@web.cache` only sets the HTTP caching header, and applies to successful responses. On a route that reads a credential it is refused at startup. `Response.cache(...)` remains for routes that decide per response. A busy public page leaves identity out of the page and has a small script ask `/me`, which stays private.

### Traces

When telemetry is configured, each request gets one span with the standard HTTP attributes, joins an incoming trace if the caller sent one, and runs its guards and `authenticate` inside that span. `@telemetry.trace` and `span` blocks work inside a route as they do anywhere, and an RFC 104 receipt can appear on the current span. With no telemetry configured, this does nothing.

### Testing

`app.test_client()` is an RFC 066 `Client` on an in-process transport with a memory cookie jar, so a test uses the real request path:

```incan
@test
@ctx_override(AppState, members=seeded_members(), sessions=web.auth.MemorySessions())
async def test_members_page_sends_browsers_to_login():
    client = build_app().test_client()
    response = await client.get("/members", headers={"Accept": "text/html"})
    assert response.status.code == 303

@test
@ctx_override(AppState, members=seeded_members(), sessions=web.auth.MemorySessions())
async def test_members_page_greets_a_member():
    client = build_app().test_client(session=MemberSession(member_id=1, device="test"))
    response = await client.get("/members")
    assert response.status.code == 200
```

`session=` starts a session without logging in, and is accepted only in a test context.

### Boundary with `std.http`

`std.http` is owned by RFC 066. This RFC does not define the HTTP client API, retry model, redirect behavior, timeout policy, or response decoding surface.

The serving side aligns with `std.http` where that improves the user experience: schema and validation conventions, `SecretStr` credentials, error and serialization conventions, and request and response vocabulary. It requires that `std.web` does not make choices that block `std.http` from providing a coherent client-side model.

### Optional organizational surfaces

The primary model remains FastAPI-like. A Django-style project layout belongs in a library over `App`, and a declarative route DSL uses RFC 160's vocabularies. These are facades over the same underlying model, not separate frameworks with separate semantics. Purely to illustrate, a vocabulary could expand a short form into the `model Member with Authenticated[...]` above.

## Reference-level explanation (precise rules)

### Scope

This RFC covers Incan's standard-library web-serving platform: `std.web`, `std.web.auth`, `std.web.cookies` and `std.password`, plus the extension boundary for other transports such as gRPC and Arrow-oriented data and RPC scenarios.

### Design constraints

1. **FastAPI-first server UX:** the primary serving experience is decorator-driven and typed.
2. **Complete server platform scope:** serving, auth, validation, middleware, lifecycle, and docs are all part of the platform.
3. **No backend leakage in public APIs:** users do not think in backend-runtime terms for ordinary API work.
4. **One platform, multiple surfaces:** FastAPI-style APIs, Django-style organization, and DSLs reduce to one underlying model.
5. **Library ownership over compiler ownership:** framework behavior belongs in the standard library and libraries, and compiler support stays primitive and general.
6. **Client compatibility without client ownership:** `std.web` aligns with RFC 066 where concepts overlap.
7. **HTTP is the primary baseline:** other transports may extend the model.

### Platform capabilities

The platform must support routing, request extraction and response conversion, schema and validation conventions, authentication and authorization, middleware pipelines, dependency provision and override, application lifecycle hooks, standardized error modeling, and documentation with OpenAPI-style metadata.

### Routes and applications

1. `get`, `post`, `put`, `patch`, `delete`, and `route` must be ordinary RFC 036 decorator factories returning route values. Handler extraction must go through RFC 141's `ArgumentsOf[F]` and type patterns, with no compiler knowledge of route decorators.
2. `App` must be built from explicit `routes`, `groups` and `on_error` lists. No decorator may register anything as a side effect, and behavior must not depend on when a decorator runs.
3. A handler may be `def` or `async def`. A `def` handler must not block other requests.
4. `Group(prefix, routes=..., guards=..., caps=..., cross_site=..., on_error=...)` must apply its settings to its routes. One route may appear in more than one application.
5. A route no application lists must be reported by a lint.
6. `@web.cache` must wrap a function and `@web.get` must wrap the result, so `@web.get` is written above it. The reverse order must be a compile error.

### Request extraction

7. `FromRequest` must be the trait for anything built from a request. `Json[T]`, `Query[T]`, `Path[T]`, `Form[T]`, `Request`, `ReturnTo`, credentials, cookies and `Authenticated` types must all adopt it.
8. A parameter of type `T` that cannot be built must stop the request with that type's error. `Option[T]` must give `None` when the value is absent and must otherwise behave as `T`. `Result[T, E]` must hand the failure to the handler, and an `E` that cannot hold that failure must be a compile error.
9. Each `FromRequest` type must define what absent means for it.
10. Each type must be built at most once per request, and every parameter asking for it must receive the same value.

### Identity and credentials

11. `Credential` must be a trait for reading a proof from a request. It must state whether a browser sends the proof automatically. `Bearer`, `ApiKey` and `SessionCookie[T]` must adopt it, holding secrets as RFC 103 `SecretStr`. `SessionCookie` must be automatic, and `Bearer` and `ApiKey` must not.
12. `@derive(Credential)` on an enum must try variants from top to bottom and use the first one present. The enum must count as automatic if any variant does.
13. `Authenticated[C]` must require a static `authenticate(credential: C)` returning `Result[Self, E]`, with `E` able to hold `Unauthorized`, and it must imply `FromRequest`.
14. An absent credential must fail `T` with `Unauthorized`, and must give `None` for `Option[T]`. A credential that is present but wrong must not fall through to a later variant.
15. When `authenticate` returns `Unauthorized` for a session cookie, the server must end that session and clear the cookie, and the request must then count as having no credential. For any other credential, the request must stop. Any other error must go to `on_error`, a 503 by default, and must not end the session.
16. The credential type must supply the security scheme in generated documentation, and the `WWW-Authenticate` challenge of a 401.

### Sessions

17. `SessionData` must require `property owner -> Option[str]`, with no default. `start_session` must refuse a model that does not adopt it.
18. `Response` must provide `start_session(data, remember_for=...)`, `update_session(data)`, `end_session()` and `end_all_sessions(owner=...)`. `start_session` must replace the session id, and `update_session` must keep it.
19. `SessionStore` must be a trait, with `DbSessions` (over `std.db`) and `MemorySessions` provided. Using sessions with no store must be refused at startup, and there must be no hidden default store.
20. A store must keep only a hash of a session id, and the raw id must not be readable by application code. `credential.handle` and `for_owner(owner)` must give a separate value that cannot authenticate. `for_owner` must also give each session's data, start time and last use, and `end(handle, owner=...)` must do nothing unless that session has that owner.
21. `expire_after` must be how long an ordinary session lasts, and `remember_for` must replace it for one session. `idle_after` must end a session unused for that long. `domain` must set the session cookie's domain.
22. An expired or unknown session cookie must count as no cookie. A session started as one model must not be readable as another.
23. If the store cannot be reached, the server must answer 503. An outage must never look like being logged out.

### Passwords

24. `std.password` must provide `hash_password`, `verify_password` and `PasswordPolicy`, without depending on `std.web`. The byte-level algorithm must live in RFC 168's `std.primitives`, and Argon2id must be the default.
25. `hash_password` and `verify_password` must not block other tasks, and must take the password as `SecretStr`. `PasswordHash` must hold the hash, algorithm and settings, must not be comparable with `==`, and must not appear in logs or default serialization.
26. `verify_password(password, record, hash_of)` must take an `Option` record, and must spend comparable time when it is `None`. It must return `PasswordCheck[T]`, with `Valid(T)`, `Upgrade(T, PasswordHash)` and `Invalid`. `Upgrade` must be returned for a hash using weaker settings than the policy, and for a recognized older algorithm such as bcrypt. Other algorithms must be able to plug in through a trait.

### Cookies

27. A type adopting `Cookie` must declare `RULES: CookieRules`, and must be readable as a parameter. `Response` must provide `set_cookie` and `clear_cookie[T]`. Values must be signed by default.
28. `CookieRules` must require `name` and `purpose`, and may set `lasts`, `domain`, `path` and looser settings. Without `lasts`, a cookie must end with the browser session. By default a cookie must be HTTPS-only, hidden from page scripts and not sent on cross-site form posts.
29. `app.cookies()` must list every declared cookie with its purpose and lifetime, including those `std.web` itself uses.
30. With a `CookiePolicy`, a cookie the policy does not allow must not be sent, reading it must give `None`, and a warning must be logged. A policy must not be able to block a `Necessary` cookie. With no policy, every cookie must be sent.
31. `request.cookies.get(name)` and `Response.set_raw_cookie(...)` must exist for cookies another system owns, and skip typing and policy.

### Errors

32. `@web.on_error` must select an error type from its first parameter, and must apply to errors from routes, guards and `FromRequest` types. The nearest handler must win: route group, then application. Without a handler, the error's standard response must be used.
33. `Response.html(body, status=...)` must set both the status and the HTML content type.
34. `.return_to(path)` must remember the path in a short-lived `Necessary` cookie, and only on a `GET`. `ReturnTo` must accept only paths on the same site.

### Guards

35. `@guard` must wrap a function into a `Guard` with parameters filled as a handler's. A guard must return `Result[None, E]` or an RFC 104 `Restriction`.
36. Guards must run in the order application, group, route, and all must pass. They must run before the request body is read, unless a guard itself asks for the body.
37. A `Restriction` must apply for the rest of the request, under RFC 104.

### Cross-site protection

38. For `POST`, `PUT`, `PATCH` and `DELETE`, `App` must apply the table above by default, including to requests with no cookie, so a site cannot log a visitor into an attacker's account. `same-site` must be refused unless the origin is in `trusted_origins`.
39. A refusal must be a `CrossSiteRefused` error. `public_origin` must be what `Origin` is compared with when it is set, because a proxy can rewrite the host.
40. `cross_site=True` must turn the check off for a route or group. On those methods, combining it with any automatic credential, including one a guard asks for, must be refused at startup, naming the route and the credential.
41. `trusted_origins` must also answer CORS for browser code on those origins, with credentials. `cross_site=True` must answer CORS for any origin, without credentials.
42. `trusted_proxies` must decide which forwarded addresses are believed, and `request.client_ip` must honor it.

### Caching

43. `std.web` must set caching headers from what each request read, under the table above. A response that asks for public caching after reading a credential must stay private, and a warning must be logged.
44. `@web.cache` must apply only to successful responses, and must be built on `Response.cache`. On a route that reads a credential, including through its guards, it must be refused at startup.

### Capabilities and traces

45. `caps=` must be accepted on routes and groups under RFC 104. It must never grant anything.
46. When telemetry is configured, each request must have one span using the standard HTTP attributes, and must join an incoming trace. Guards and `authenticate` must run inside it. Without telemetry, this must add nothing.

### Testing

47. `app.test_client(session=...)` must return an RFC 066 `Client` using an in-process transport and a memory cookie jar, and must behave as a same-origin browser. `session=` must be accepted only in a test context.

### Local servers, output and request size

48. A server bound only to loopback addresses must, by default, refuse a request whose `Host` header names anything other than a loopback name or address with the bound port. A page that rebinds its own domain to `127.0.0.1` shares the attacker's origin, so `Origin` and `Sec-Fetch-Site` alone cannot catch it. An application may accept further hosts only by naming them.
49. Interpolation into `web.Html` is allowed only in data positions: element text, and quoted values of ordinary attributes, each escaped for its position.
    - **URL attributes:** an attribute such as `href`, `src`, `action` or `formaction` also takes only a relative URL, or one whose scheme is `http`, `https` or `mailto`, and refuses any other. Escaping alone does not make a `javascript:` URL safe.
    - **Refused positions:** interpolation into a script, a style element, an event-handler attribute, a `style` attribute, an unquoted attribute, a tag or attribute name, or a comment must be refused. The refusal comes at compile time when the template is static, and before any output otherwise.
    - **Script data:** data a script needs goes through a JSON data block that `std.web` serializes and escapes, never into script text.
    - **Raw markup:** markup that is not to be escaped comes through a distinct raw type. It goes only where element content is allowed, never into a refused position.
50. Request bodies, header sections and multipart parts must be bounded before they are read, with limits an application can set. A request beyond a limit must be refused without reading the excess.

### Middleware and lifecycle

The platform must support deterministic middleware ordering, short-circuiting, enrichment and error transformation, startup and shutdown hooks, and integration of background tasks and long-lived resources.

### Documentation and contracts

The platform must support schema-aware request and response contracts, OpenAPI-style docs generation, explicit metadata for summaries, tags, examples and security requirements, and stable error contract documentation where possible.

### Transport extensions

HTTP serving is the primary target of this RFC. Other transport RFCs can extend the platform toward gRPC-style service transports and Arrow-oriented data and RPC transports. The intent is not to force all transports into one fake-HTTP abstraction, but to avoid designing `std.web` in a way that blocks them.

### Conformance workloads

| Workload | Required evidence |
| --- | --- |
| Routes | Routes work as `def` and `async def`, through a module binding, in groups and in two applications. An unlisted route is linted. |
| Extraction | `T`, `Option[T]` and `Result[T, E]` behave as specified for bodies, forms, query values, cookies and credentials, and a type is built once per request. |
| Identity | `Bearer`, `ApiKey`, `SessionCookie` and a `@derive(Credential)` enum all work, a bad credential stops the request, and adding an enum variant fails to compile until handled. |
| Removed member | A deleted member becomes a visitor on a public page, and their session is ended. A bad token still gets a 401. An outage keeps the session. |
| Sessions | Login, logout, `remember_for`, `update_session`, log out everywhere, listing devices and ending one all work. A forgotten `owner` does not compile. |
| Session secrets | A stored session database contains no working cookie value, no application code can read a raw id, and one owner cannot end another's session. |
| Passwords | A correct password, a wrong one, an unknown email and an old hash give the right `PasswordCheck`, and an unknown email spends comparable time. |
| Cookies | Typed cookies read, set and clear with safe defaults, `app.cookies()` lists them all, and a `CookiePolicy` holds back all but `Necessary`. |
| Errors | `on_error` covers routes, guards and credentials, and the nearest handler wins. `ReturnTo` refuses a path on another site. |
| Guards | Guards run in order and before the body is read, and a `Restriction` stops a route that forgot its ownership check. |
| Cross-site | Each row of the table behaves as stated, login is protected with no cookie, and `cross_site=True` with an automatic credential refuses to start. |
| Caching | A credential route and a `Set-Cookie` response are never cacheable, and `@web.cache` on a credential route refuses to start. |
| Traces | One span per request joins an incoming trace, and nothing is added without telemetry. |
| Testing | `test_client` runs the real request path under `ctx_override`, and `session=` is refused outside a test context. |

### Compatibility and migration

Route values replace `@route`, compiler-collected routes and `App.run()`. `Response`, `Json[T]`, `Html`, `Query[T]` and `Path[T]` keep their meaning, so a handler body does not change. `@route` follows RFC 150's deprecation path, with a diagnostic showing the new form.

## Design details

### Primary and secondary surfaces

The FastAPI-like `std.web` surface is the reference mental model. Django-style organizational layers, declarative DSLs and transport-specific facades are secondary surfaces, and they lower to it or align tightly with it rather than growing independent semantics.

### What belongs where

Placement follows how often something changes.

| Level | What goes there |
| --- | --- |
| `std.primitives` | Secure random bytes and the byte-level password-hashing algorithm |
| The standard library | Everything this RFC specifies |
| A higher-level framework | Consent, generated notices, JWT validation, OAuth login, account flows, login-attempt policy, role systems |
| Other libraries | Further session stores and unusual login providers |

A plain application must be able to log people in with the standard library alone. The levels above add what changes more often than a compiler release.

### Why identity is a parameter

In Django, middleware sets `request.user` on every request, an anonymous visitor gets an `AnonymousUser`, and a view needs `@login_required` to refuse them, so forgetting the decorator leaves the view open. Here a route that uses the caller cannot run without one, because the caller is its parameter. The `authenticate` function is the same idea as a Django authentication backend. It is closest to FastAPI's dependencies and to extractors in Rust's Axum, without a default value on every parameter.

### Why routes are values

A module holds only declarations, so `app = App()` cannot be written at module level. A static application would register routes as a side effect of decorators and could not be built per test. Compiler collection is shorter, but it makes route decorators a compiler special case. Lists are what `tools=[...]` already does in RFC 156, and a vocabulary can shorten them.

### Why the check uses browser headers

A hidden token in every form is the older answer to cross-site requests, and it needs a cookie and a template step on every form. `Sec-Fetch-Site` and `Origin` need neither. Go's standard library adopted the same approach in version 1.25.

### Why sessions live on the server

A signed cookie needs no storage, but it stays valid until it expires, so nobody can be logged out early. A server-side record supports logout, log out everywhere and listing devices, and the cookie stays small.

### Relationship to existing RFCs

- **RFC 023** establishes the stdlib/runtime handoff baseline, and this RFC removes more user-facing interop leakage from the final experience.
- **RFC 033** owns shared state, so `std.web` needs no state mechanism.
- **RFC 035** is important because function values are a natural part of handler, middleware, and decorator systems.
- **RFC 036** is foundational because proper decorators are central to the `@web.get(...)` model.
- **RFC 104** owns capabilities, limits and receipts, and **RFC 095** owns spans.
- **RFC 141** lets route decorators read parameter types as library code.
- **RFC 160** owns DSL forms, which prefer vocabularies over new compiler special cases.
- **RFC 031** matters because framework growth should live comfortably in the library ecosystem.

### Sub-area RFC boundary

A sub-area RFC may add precision to docs and schema generation rules, transport-specific integrations such as gRPC, or declarative DSL syntax and desugaring. It inherits this RFC's product direction rather than reopening it.

## Alternatives considered

- **Stay with the current hybrid model:** too much leakage and too little coherence.
- **A static application with `@app.get`:** registration becomes a side effect, tests share one application, and a static initializer cannot change another static.
- **Compiler-collected routes:** the shortest form, but compiler magic for a library concern.
- **`Depends(current_user)` as in FastAPI:** works, but needs a default value on every parameter that wants the user.
- **A login decorator and `request.user` as in Django:** forgetting the decorator leaves a route open.
- **Application state passed as a type parameter:** catches a mismatch at compile time, but adds a type to every `Authenticated[...]`, and RFC 033's `ctx` covers state with no new mechanism.
- **A dependency registry by type:** nothing to declare, but a missing entry only shows up when a request hits it.
- **`SessionCookie | Bearer` for two methods:** shorter, but a union has no order, and something must decide when both arrive.
- **`/login?next=...`:** the query string is what makes the redirect attack possible.
- **`Form[T]` with `is_valid()`:** familiar from Django, but it only solves forms, and a route that forgets to ask carries on with bad data.
- **Consent enforced by `std.web`:** consent rules change often, and a release tied to the compiler cannot follow them.
- **JWT and OAuth in `std.web`:** the same reason, and they plug in without any change here.
- **Make the compiler own the web framework semantics:** that would work against stdlib and library evolution.
- **Make Django-style organization the primary model:** FastAPI-style ergonomics are the better default for modern API development.

## Drawbacks

- An application lists every route, and a forgotten route gives a 404 until the lint reports it.
- `model Member with Authenticated[...]` is longer than a decorator. A vocabulary can shorten it without changing the foundation.
- A public page that asks for identity can never be cached, so a busy page has to separate the two.
- The safe defaults refuse some combinations outright, including `cross_site=True` with a session cookie, with no override.
- Plain HTTP falls back to the weaker `Origin` check, because browsers only send `Sec-Fetch-Site` to HTTPS sites.
- Docs generation and DSL forms stay directional here, and their sub-areas need their own precision.

## Layers affected

- **Stdlib / runtime**: `std.web`, `std.web.auth`, `std.web.cookies` and `std.password`, without leaking backend crate APIs as the primary contract, and with secure random bytes and the password algorithm in `std.primitives`.
- **Language surface**: decorators and types reached through a module binding must work like any others. Nothing else requires compiler knowledge of the web.
- **Execution handoff**: implementations preserve the language-level web semantics while mapping onto the chosen runtime substrate.
- **Lints**: a route no application lists.
- **Docs / tooling**: `incan inspect` and generated docs show routes, their guards, capabilities, caching and security schemes.

## Design Decisions

1. **Identity is a handler parameter,** through `Authenticated[C]` and one `authenticate` function. An absent credential is `Unauthorized` for `T` and `None` for `Option[T]`.
2. **A credential is how proof travels, and `authenticate` is how it is checked.** `std.web` ships `Bearer`, `ApiKey` and `SessionCookie`. Two methods on one route is a `@derive(Credential)` enum, tried top to bottom.
3. **A wrong credential stops the request,** except that `Unauthorized` on a session cookie ends the session and the caller becomes a visitor. Other errors, including an outage, keep the session.
4. **Sessions live on the server,** with the store in a `ctx` and no default. The id is stored hashed and never exposed, `handle` is safe to show, and `SessionData` states the owner. The cookie ends with the browser unless `remember_for` is used.
5. **Password hashing is split:** the algorithm in `std.primitives`, and `std.password` for the everyday API.
6. **Cookies are a general typed layer,** with a required `purpose`. `std.web` makes no consent decision, and offers `app.cookies()` and `CookiePolicy` to a higher layer.
7. **Shared state is an RFC 033 `ctx`,** so `std.web` adds no state mechanism. Several contexts cover several applications.
8. **An application is built from values.** `@web.get` and `@web.post` turn a function into a route value, and `App(routes=[...], groups=[...])` lists what it serves, in a function, because a module holds only declarations. Nothing depends on when a decorator runs, tests build their own application, and one route can appear in more than one application. A route no application lists is reported by a lint.
9. **`T`, `Option[T]` and `Result[T, E]` are one rule** for anything built from a request, implemented with RFC 141's type patterns.
10. **`@web.on_error` turns errors into responses,** which also covers the login redirect. The return path lives in a cookie rather than the query string.
11. **Guards are the one authorization mechanism,** and can return a `Restriction`. Role and permission systems live above the standard library.
12. **Cross-site protection uses browser headers,** is on by default, and refuses `same-site` unless the origin is trusted. `cross_site=True` with an automatic credential refuses to start.
13. **Caching is private when identity or cookies are involved,** and `@web.cache` is the explicit way to choose otherwise.
14. **`caps=` is optional but true,** on routes and groups, under RFC 104.
15. **Validation is split by responsibility:** compile-time validation covers names, signatures, decorator shape, metadata shape, and other statically knowable contracts, and runtime validation covers request payloads, path and query values, auth decisions, and failures that depend on incoming data.
16. **Docs and schemas are inferred** from checked handler signatures, models, credentials and response declarations, with explicit metadata for summaries, tags, examples, security requirements, and cases where inference would be ambiguous or misleading.
17. **Django-style organization is a secondary surface.** `std.web` owns the serving primitives and grouping, and project layout belongs in libraries over `App`.
18. **RFC 037 owns HTTP serving.** gRPC, Arrow-oriented, or other transport RFCs may reuse concepts such as `App`, `Route`, `Schema`, `Guard`, and lifecycle hooks, but must not be forced into a fake-HTTP abstraction or make RFC 037 responsible for their wire-level details.
