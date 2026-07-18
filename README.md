# JITForge

JITForge turns natural-language intent, representative inputs, and optional strict examples into validated, versioned Unix capabilities with an auditable invocation path.

The model participates only during synthesis and repair. Once a capability is published, normal calls execute the selected artifact directly without invoking the model again.

```text
Intent + Input Sample + Strict Example
  → Contract Review
  → Source + Tests
  → Sandbox Validation
  → Immutable Artifact
  → name@revision / stable
  → CLI, Web, Shell, Agent, or CI invocation
```

The current release line is `0.1.0` alpha. It is suitable for self-hosted evaluation, but it is not yet a public service for untrusted multi-tenant workloads.

## Intended Use

JITForge is designed for short-lived, stateless Unix filters with explicit inputs and outputs. Good use cases include:

- parsing operational command output and producing reports;
- cleaning, validating, or converting JSON, CSV, and text;
- replacing small pieces of glue logic repeated across pull requests or CI jobs;
- publishing capabilities that Shell users and Agents can share, pin, and audit;
- performing a small number of explicitly approved public HTTPS queries.

Generated programs currently use a fixed, single-file Python 3 standard-library runtime. Long-running services, browser automation, arbitrary third-party dependencies, persistent state, and capabilities that require private user credentials are outside the current scope.

## Core Semantics

- Data piped to `register` is an **Input Sample**. It demonstrates the real input shape but does not imply an expected output.
- `--example 'INPUT => OUTPUT'` supplies a **Strict Example** and becomes a validation assertion. If JITForge detects that an example is incorrect, the job pauses for explicit approval while preserving the original value in the audit record.
- Before source generation, a Contract defines input, output, failure semantics, and the test plan, then passes through an independent review.
- A revision becomes `ready` only after Contract Review, user tests, generated tests, and actual sandbox execution all succeed.
- `name@revision` identifies an immutable revision. A bare name resolves through the current `stable` pointer.
- Invocation follows Unix conventions: business output goes to stdout, diagnostics go to stderr, and failures return a non-zero exit code.

Model-generated tests and the independent Verifier do not constitute a formal correctness proof. They are release gates that complement, rather than replace, user assertions and real sandbox results.

## Architecture

```text
                         ┌──────── PostgreSQL Registry
                         │
jit CLI ──HTTP──> jit-server ──private gRPC──> jit-worker
                         │                         │
                         │                         ├── synthesis Agent / Verifier
                         │                         ├── Artifact Store
                         │                         └── Docker + runsc ──> capability
                         │
                         └── embedded Web Console

Internet ──TLS proxy / tunnel──> Nginx ──> Console, API, and health endpoints
```

`jit-server` owns the HTTP API, authentication, browser sessions, registry access, and job status. It does not hold model credentials or access the Docker socket. `jit-worker` is the only service with model configuration and Docker access, and it does not publish a host port by default.

PostgreSQL stores Contracts, revisions, artifact digests, validation evidence, job checkpoints, traces, approvals, and invocation metadata. Invocation stdin and stdout bodies are not stored by default.

## Quick Start

### Requirements

- Linux with Docker Engine and Docker Compose;
- [gVisor `runsc`](https://gvisor.dev/docs/user_guide/install/) registered as a Docker runtime;
- Rust 1.97 and `protoc`;
- for real synthesis, an OpenAI Chat Completions-compatible endpoint with native tool-call support.

Verify that Docker can use `runsc`:

```bash
runsc --version
docker info --format '{{json .Runtimes}}'
docker run --rm --runtime=runsc python:3.13-alpine python3 -c 'print("gvisor-ok")'
```

### Exercise the Full Path with the Fixture Synthesizer

The Fixture Synthesizer requires no model API key and accepts only test registrations marked with `[fixture:...]`. It verifies that PostgreSQL, Worker, Server, Runner, Artifact storage, and the CLI are connected; it does not represent real model quality.

Create a local Compose environment file:

```bash
cp .env.example .env
```

Edit `.env`, replace at least the two tokens, and select fixture mode:

```dotenv
JITFORGE_TOKEN=choose-a-local-client-token
JITFORGE_WORKER_TOKEN=choose-a-different-worker-token
JITFORGE_SYNTHESIZER_MODE=fixture
```

`.env` is ignored by Git and must not be committed. Build the control plane and CLI, then start the complete Compose profile:

```bash
cargo build --locked --release -p jit-cli -p jit-server -p jit-worker
docker compose --profile containerized up --build -d
```

Check service health:

```bash
curl -f http://127.0.0.1:8090/healthz
curl -f http://127.0.0.1:8090/readyz
docker compose ps
```

Point the CLI at the local Gateway, using the same client token as `.env`:

```bash
export JITFORGE_SERVER=http://127.0.0.1:8090
export JITFORGE_TOKEN=choose-a-local-client-token
```

Register and invoke a fixture capability:

```bash
printf 'Hello Cloud Native\n' |
  target/release/jit register slugify \
    '[fixture:slugify] Convert UTF-8 text to a lowercase URL slug' \
    --example 'Hello Cloud Native => hello-cloud-native'

printf 'Hello Cloud Native\n' | target/release/jit call slugify
```

Expected stdout:

```text
hello-cloud-native
```

The Gateway root responds with `307 Temporary Redirect` and sends the browser to the Web Console:

```text
http://127.0.0.1:8090/
  → http://127.0.0.1:8090/ui/
```

### Switch to a Real Model

Change the synthesis mode in `.env` and configure the model endpoint:

```dotenv
JITFORGE_SYNTHESIZER_MODE=openai
JITFORGE_LLM_BASE_URL=https://provider.example/v1
JITFORGE_LLM_API_KEY=replace-me
JITFORGE_LLM_MODEL=replace-me
JITFORGE_LLM_VERIFIER_MODEL=replace-me
JITFORGE_LLM_THINKING=auto
```

The Coder and Verifier use separate contexts. They may use the same model, but `JITFORGE_LLM_VERIFIER_MODEL` must still be set explicitly. The provider must support `tools`, `tool_choice`, assistant `tool_calls`, and subsequent tool-result messages.

Recreate the application containers after changing model configuration:

```bash
docker compose --profile containerized up -d --force-recreate worker server nginx
```

## CLI

```text
jit register NAME INTENT [--example 'INPUT => OUTPUT'] [--no-wait]
jit status JOB_ID
jit answer JOB_ID TEXT
jit answer JOB_ID --approve
jit cancel JOB_ID
jit list [QUERY]
jit search QUERY
jit inspect NAME[@REVISION]
jit call NAME[@REVISION] [--input TEXT | --file PATH] [-- TOOL_ARGS...]
jit revoke NAME@REVISION --reason TEXT
```

`register` waits by default until the job reaches `ready`, `rejected`, or a state that requires user input. With `--no-wait --json`, it returns a Job ID immediately; use `status` to follow the job. `answer` supplies clarification, approves an example correction, or approves an HTTP Capability. `cancel` stops queued, running, or paused jobs.

`call` forwards the remote stdout, stderr, and exit code. Automation should pin `name@revision`; interactive use may choose the bare name and its `stable` pointer.

Configuration precedence is CLI arguments, environment variables, then the configuration file. The default file is `~/.config/jitforge/config.toml`; `$XDG_CONFIG_HOME/jitforge/config.toml` and `JITFORGE_CONFIG` are also supported. On Unix, a configuration file containing tokens or API keys must have mode `0600`.

## Web Console and Gateway Routes

The complete Compose profile exposes one Nginx entry point:

```text
/                              307 redirect to /ui/
/ui/                           Web Console
/v1/                           HTTP API
/healthz                       process health
/readyz                        PostgreSQL and Worker readiness
```

The Console supports login, capability listing and inspection, registration, job handling, invocation, revocation, HTTP Capability management, and system status. Browsers use an HttpOnly, SameSite=Strict session cookie; mutating requests must also provide the matching `X-JitForge-Csrf` token.

## Synthesis and Publication

The synthesis Agent uses constrained native tool calls and accepts one action per model turn. Current per-job limits include 24 model turns, 4 source revisions, 3 generated-test corrections, and 3 sandbox probes. Web searches and HTTP probes have separate budgets.

A typical job proceeds as follows:

1. Derive a Contract and test plan from the Intent, Input Sample, and Strict Examples.
2. Run an independent review for requirement drift, invalid oracles, and sample hard-coding.
3. Permit source generation or editing only after the Contract passes review.
4. Execute input samples, user tests, generated tests, and probes through the runsc Runner.
5. Pause for clarification, example correction, or HTTP Capability approval when required, then resume from a checkpoint.
6. Store a content-addressed artifact, publish an immutable revision, and update the `stable` pointer only after every gate passes.

## HTTP Capabilities

Capabilities have no network access by default. When current public data is genuinely required, the synthesis Agent may request a narrowly scoped HTTPS GET grant defined by host, port 443, path prefix, and allowed query keys.

After user approval, the grant is published with the artifact. The runtime policy permits generated code to call matching destinations through `jitforge_http.get`; IP literals, private address resolution, credential-bearing URLs, arbitrary ports, and raw socket access are rejected. Revoking an approval prevents artifacts that reference it from making further networked calls.

Live HTTP access also requires an explicit deployment setting:

```dotenv
JITFORGE_HTTP_MODE=direct
```

Leave the default `disabled` mode in place when networked capabilities are unnecessary. Direct mode relies on the runtime policy and sandbox as defense in depth; it is not a formal security boundary against deliberately hostile generated code.

## Security Boundaries

Generated capabilities run through Docker and runsc with the following controls:

- UID/GID 65532 and a read-only root filesystem;
- `cap-drop=ALL` and `no-new-privileges`;
- a 32 MiB `noexec` tmpfs at `/tmp`;
- 128 MiB of memory, 0.5 CPU, 16 processes, and 64 file descriptors;
- 1 MiB limits for stdout and stderr plus a hard timeout;
- `network=none` by default.

These controls apply to generated capability containers, not necessarily to the Server, Worker, and Nginx containers. `runsc` is an isolation layer, not a complete security proof. The Worker mounts the Docker socket and remains a privileged trust boundary.

Authentication currently uses one shared Bearer token. The PostgreSQL credentials in Compose are development defaults and must be replaced for deployment. The Server provides HTTP only, so an external TLS terminator is required for public access. In `0.1.0`, the Web session cookie does not yet carry the `Secure` attribute; do not expose the Console directly to an untrusted network until that is addressed.

## Repository Layout

```text
apps/jit-cli              Rust CLI
apps/jit-server           HTTP API, Session/CSRF, Registry control plane, embedded Console
apps/jit-worker           synthesis Agent, Verifier, Runner, and publication pipeline
crates/jit-*              Artifact, Config, Domain, Protocol, and Storage crates
web/console               HTML/CSS/JS embedded into jit-server
runtimes/python-stdlib-v2 fixed Python runtime for generated capabilities
deployments               Compose, Nginx, and SearXNG configuration
migrations                PostgreSQL migrations
```

## Development Checks

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
JITFORGE_TOKEN=test JITFORGE_WORKER_TOKEN=worker-test \
  docker compose --profile containerized config --quiet
```

For day-to-day development, start only PostgreSQL, SearXNG, and the runtime images with Compose, then run the Worker and Server on the host:

```bash
docker compose up --build -d
JITFORGE_WORKER_TOKEN=worker-test \
  JITFORGE_SYNTHESIZER_MODE=fixture \
  cargo run -p jit-worker
```

In another terminal:

```bash
JITFORGE_TOKEN=client-test \
  JITFORGE_WORKER_TOKEN=worker-test \
  cargo run -p jit-server
```

## Project Status

JITForge is an alpha project. The API, artifact format, configuration, and deployment model may change throughout the `0.1.x` series.

## License

JITForge is licensed under the [GNU Affero General Public License v3.0](LICENSE), using the SPDX identifier `AGPL-3.0-only`.
