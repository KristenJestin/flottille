# Flottille's hub, as a container image (docs/releases.md, "What CI publishes"): its static binary
# and nothing else, so that there is no base to update and nothing to run but the hub. The binary
# is built before, for the image's architecture (musl); this file builds nothing.
#
# The build context holds `flottille-hub` (that binary) and an empty `state` folder:
#   docker build -f crates/flottille-hub/Dockerfile <context>
#
# It listens plain on every address of the container, for a reverse proxy that holds the
# certificate: publish its port on the host's loopback only (compose.example.yml does). For TLS on a
# certificate given, mount it and give the arguments (`--tls-cert`, `--tls-key`) as the command.
#
# GHCR makes a package private when it first appears, unless the image names its repository: the
# build is given it (`--build-arg SOURCE=<the repository's URL>`).
FROM scratch
ARG SOURCE
LABEL org.opencontainers.image.source=${SOURCE}
COPY --chmod=0755 flottille-hub /flottille-hub
COPY --chown=65532:65532 --chmod=0700 state/ /var/lib/flottille-hub/
USER 65532:65532
EXPOSE 7448
VOLUME /var/lib/flottille-hub
ENTRYPOINT ["/flottille-hub"]
CMD ["--listen", "0.0.0.0:7448", "--plain", "--state", "/var/lib/flottille-hub"]
