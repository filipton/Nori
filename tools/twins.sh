#!/usr/bin/env bash
# Writes the twins' test vectors: every generator under crates/*/testdata/twins/*.kt is compiled with
# the Kotlin compiler Gradle already downloaded (the app's own version) and run on the host JVM, and what
# it prints is the table next to it (Foo.kt -> foo.tsv). The Rust tests read those tables, so the Kotlin
# original and its Rust twin are held to the same answers. Run it again after changing either side.
#
#   tools/twins.sh             every generator
#   tools/twins.sh path/X.kt   one
set -euo pipefail
cd "$(dirname "$0")/.."

g="$HOME/.gradle/caches/modules-2/files-2.1"
jar() { find "$g/$1" -name "$2" -not -name '*sources*' | sort | tail -n 1; }
kotlin=2.4.20
stdlib=$(jar "org.jetbrains.kotlin/kotlin-stdlib/$kotlin" "kotlin-stdlib-$kotlin.jar")
compiler=$(jar "org.jetbrains.kotlin/kotlin-compiler-embeddable/$kotlin" '*.jar')
script=$(jar "org.jetbrains.kotlin/kotlin-script-runtime/$kotlin" '*.jar')
daemon=$(jar "org.jetbrains.kotlin/kotlin-daemon-embeddable/$kotlin" '*.jar')
reflect=$(jar org.jetbrains.kotlin/kotlin-reflect 'kotlin-reflect-2.4*.jar')
coroutines=$(jar org.jetbrains.kotlinx/kotlinx-coroutines-core-jvm 'kotlinx-coroutines-core-jvm-1.10*.jar')
annotations=$(jar org.jetbrains/annotations 'annotations-13.0.jar')
for j in "$stdlib" "$compiler" "$script" "$daemon" "$reflect" "$coroutines" "$annotations"; do
    [[ -f $j ]] || { echo "missing a Kotlin compiler jar; build the app once so Gradle fetches them" >&2; exit 1; }
done

out=$(mktemp -d)
trap 'rm -rf "$out"' EXIT
if (($#)); then gens=("$@"); else mapfile -t gens < <(ls crates/*/testdata/twins/*.kt); fi
for kt in "${gens[@]}"; do
    name=$(basename "$kt" .kt)
    table="$(dirname "$kt")/$(echo "$name" | sed 's/\([a-z0-9]\)\([A-Z]\)/\1_\2/g' | tr 'A-Z' 'a-z').tsv"
    rm -rf "$out/classes"
    java -XX:+IgnoreUnrecognizedVMOptions --enable-native-access=ALL-UNNAMED --sun-misc-unsafe-memory-access=allow \
        -cp "$compiler:$stdlib:$script:$reflect:$daemon:$coroutines:$annotations" \
        org.jetbrains.kotlin.cli.jvm.K2JVMCompiler -no-stdlib -no-reflect -nowarn -cp "$stdlib" \
        -d "$out/classes" "$kt"
    java -cp "$out/classes:$stdlib" "${name}Kt" > "$table"
    echo "$table: $(wc -l < "$table") rows"
done
