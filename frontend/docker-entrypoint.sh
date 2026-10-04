#!/bin/sh
set -eu
export NGINX_RESOLVER="$(awk '/^nameserver/ { print $2; exit }' /etc/resolv.conf)"
exec /docker-entrypoint.sh "$@"
