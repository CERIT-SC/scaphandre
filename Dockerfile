FROM rust:1.78 as planner
WORKDIR app

RUN cargo install cargo-chef
COPY . .

# Analyze dependencies
RUN cargo chef prepare  --recipe-path recipe.json

FROM rust:1.78 as cacher
WORKDIR app
RUN cargo install cargo-chef
COPY --from=planner /app/recipe.json recipe.json

# Cache dependencies
RUN cargo chef cook --release --recipe-path recipe.json

FROM rust:1.78 as builder
WORKDIR app
COPY . .

# Copy over the cached dependencies
COPY --from=cacher /app/target target
COPY --from=cacher $CARGO_HOME $CARGO_HOME
RUN cargo build --release

FROM ubuntu:22.04 as runtime
WORKDIR app

RUN apt-get update \
    && DEBIAN_FRONTEND="noninteractive" apt-get install -y ca-certificates tzdata libssl3 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/scaphandre /usr/local/bin

# Probably due to "/proc:/proc" bind mount, signal handling is not working when executing scaphandre directly.
# Using a custom entrypoint script to handle signals properly.
COPY docker-entrypoint.sh /docker-entrypoint.sh
RUN chmod +x /docker-entrypoint.sh
ENTRYPOINT [ "/docker-entrypoint.sh" ]
