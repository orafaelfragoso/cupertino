# The job summary of a `cargo xtask ci` run, in Markdown, from target/xtask/ci-latest.json:
#
#   jq -r -f .github/ci-summary.jq target/xtask/ci-latest.json >> "$GITHUB_STEP_SUMMARY"
#
# The plan (scope, base, fallback, changed and affected packages) and one table row per check. Fields the report does
# not have are shown as `?`, so a change of the report's shape never fails the job that writes the summary.

def names:
  if . == null then "none"
  elif type == "array" then
    (map(if type == "object" then (.name // .package // tostring) else tostring end)
     | if length == 0 then "none" else join(", ") end)
  else tostring end;

def text: if . == null then "?" elif type == "string" then . else tojson end;

def duration:
  if .duration_ms != null then "\(.duration_ms / 1000 | . * 10 | round / 10) s"
  elif .duration_s != null then "\(.duration_s) s"
  elif .duration != null then (.duration | text)
  else "" end;

def checks:
  (.checks // .results // [])
  | if type == "object" then to_entries | map((.value | if type == "object" then . else {status: .} end) + {name: .key})
    else . end
  | .[];

"### cargo xtask ci",
"",
"- profile: \(.profile | text)",
"- scope: \(.scope | text)",
"- base: \(.base_ref | text) (merge base \(.merge_base | text))",
"- status: \(.status | text)\(if .error then ": " + .error else "" end)",
"- fallback: \(.fallback // "none" | text)",
"- changed packages: \(.changed_packages // .changed | names)",
"- affected packages: \(.affected_packages // .affected | names)",
"",
"| check | result | duration |",
"|---|---|---:|",
(checks
 | "| \(.name // .id // "?" | text)\(if .config then " (\(.config | text))" else "" end)"
   + " | \(.status // .result // .outcome // "?" | text) | \(duration) |")
