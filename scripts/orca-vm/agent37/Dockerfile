# The image every Tolkster workspace on Agent37 runs: the public worker base (Node 24, git and
# Orca's Linux app, signed in to nothing; github.com/Tolkien-Works/tolkster-worker-base), plus
# Tolkster's Rust as its repo layer. Built in Agent37's cloud by
# `node scripts/orca-vm/agent37-base.ts image`.
# Everything goes outside /home/node: the instance's volume masks it at runtime.

# The base's tag for one Orca release, pinned so a new Orca comes in by a reviewed raise here.
ARG ORCA_VERSION=1.4.235
FROM ghcr.io/tolkien-works/tolkster-worker-base:orca-${ORCA_VERSION}

USER root

# Rust for cli/, and the C compiler `ring` builds with: the toolchain under /usr/local, the
# registry and build caches in the home volume (CARGO_HOME is set at runtime by the scripts
# that build).
ENV RUSTUP_HOME=/usr/local/rustup
RUN apt-get update \
  && apt-get install -y --no-install-recommends gcc libc6-dev \
  && rm -rf /var/lib/apt/lists/* \
  && curl -fsSL https://sh.rustup.rs \
    | env CARGO_HOME=/usr/local/cargo sh -s -- -y --no-modify-path --profile minimal --component clippy \
  && chmod -R a+rX /usr/local/rustup /usr/local/cargo \
  && /usr/local/cargo/bin/cargo --version

ENV PATH=/usr/local/cargo/bin:${PATH}
USER node
# Agent37 counts an instance up only while its main process runs, and the base's default is a
# shell that exits at once. A wake restarts the container, so this runs the boot script `boot`
# (agent37.ts) keeps on the home volume, if any, then waits. What a boot does lives in that
# script, never here, so this line never needs a new image.
CMD ["sh", "-c", "test -f /home/node/.orca-serve.sh && sh /home/node/.orca-serve.sh; exec sleep infinity"]
