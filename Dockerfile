# syntax=docker/dockerfile:1
FROM timayz/timada:builder AS builder

COPY . .

# Builds the binary and, next to it, the `assets` directory it serves its
# stylesheet, scripts and icons from.
RUN topcoat asset bundle -p timada-dashboard --release

# The directories the image needs besides the binary, laid out as they will
# sit in it: copying their parent keeps each one's owner and mode, which
# copying a directory itself does not.
RUN mkdir -p /out/tmp /out/var/lib/timada \
 && chown 10001:10001 /out/var/lib/timada \
 && chmod 1777 /out/tmp

FROM scratch

COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY --from=builder /etc/passwd /etc/passwd
COPY --from=builder /etc/group /etc/group
COPY --from=builder /app/target/release/timada-dashboard /usr/bin/timada-dashboard
# The binary looks for its bundle in the `assets` directory beside itself.
COPY --from=builder /app/target/release/assets /usr/bin/assets
# /tmp, and /var/lib/timada: where the built-in configuration keeps the store,
# the pictures and the keys (`data/…` under the working directory); mount a
# volume here, or name other paths in the configuration.
COPY --from=builder /out/ /

USER 10001:10001
WORKDIR /var/lib/timada

# The web app, the store API and the other nodes of a cluster.
EXPOSE 3000 4321 7000

ENTRYPOINT [ "timada-dashboard" ]
CMD ["serve"]
