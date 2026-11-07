FROM rust:1.95-bookworm
WORKDIR /app
COPY . .
RUN cargo build --release --locked -p cgg-socket -p cgg-harness
EXPOSE 8805
CMD ["target/release/cgg-harness", "dev", "--keep"]
