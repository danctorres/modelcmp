#!/bin/sh
# Usage: pkg/render.sh v1.2.3 SHA256SUMS TEMPLATE > OUT
# Replaces @VERSION@ (without the "v") and @SHA256_<target>@ in TEMPLATE.
set -eu
version=${1#v}
# The template's own two lines say what it is, which the rendered formula is not.
script="1,2d;s/@VERSION@/$version/g"
while read -r sum file; do
  target=${file#modelcmp-}
  target=${target%-"$1".tar.gz}
  script="$script;s/@SHA256_$target@/$sum/g"
done < "$2"
out=$(sed "$script" "$3")
if printf '%s\n' "$out" | grep -q '@[A-Za-z0-9_-]*@'; then
  echo "unfilled placeholder in $3" >&2
  exit 1
fi
printf '%s\n' "$out"
