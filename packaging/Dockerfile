# GAZE in a container (Linux, x86-64).  See docs/LINUX.md.
#
#   docker build -f packaging/Dockerfile -t gaze .
#   docker run -d --name gaze --restart unless-stopped -p 8080:8080 -p 8443:8443 \
#     -v gaze-data:/data gaze
#
# Everything (settings, accounts, recordings) is in the /data volume.
FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release --bin nvr

FROM debian:bookworm-slim
# ffmpeg does the video; smartmontools is optional (drive health, needs
# --privileged or --device for the drives); ca-certificates for downloads.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ffmpeg ca-certificates smartmontools \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/nvr /usr/local/bin/gaze
WORKDIR /data
VOLUME /data
EXPOSE 8080 8443
ENTRYPOINT ["/usr/local/bin/gaze", "--dir", "/data"]
