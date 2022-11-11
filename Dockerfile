FROM rust:1.65 AS builder
COPY . .
RUN cargo build --release

FROM rust:1.65
COPY --from=builder ./target/release/snarkvm_abci ./target/release/snarkvm_abci
CMD ["/target/release/snarkvm_abci"]