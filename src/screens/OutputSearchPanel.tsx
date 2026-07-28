import { useCallback, useEffect, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";

/** Mirrors `src-tauri/src/output_search.rs`. */
interface OutputMatch {
  line_no: number;
  line: string;
  context_before: string[];
  context_after: string[];
}
interface SearchResult {
  matches: OutputMatch[];
  total: number;
  truncated: boolean;
  lines_scanned: number;
}
interface SearchPresetInfo {
  id: string;
  label: string;
  query: string;
  regex: boolean;
  case_sensitive: boolean;
  description: string;
}

/** Wrap the first match of `query` in `line` with a highlight span. Literal by
 *  default; for regex, only the first match is highlighted (good enough). Falls
 *  back to the plain line if the pattern can't be built or doesn't match. */
function highlight(
  line: string,
  query: string,
  regex: boolean,
  caseSensitive: boolean,
): ReactNode {
  if (!query) return line;
  let start = -1;
  let len = 0;
  try {
    if (regex) {
      const re = new RegExp(query, caseSensitive ? "" : "i");
      const m = re.exec(line);
      if (m && m[0].length > 0) {
        start = m.index;
        len = m[0].length;
      }
    } else {
      const hay = caseSensitive ? line : line.toLowerCase();
      const needle = caseSensitive ? query : query.toLowerCase();
      const i = hay.indexOf(needle);
      if (i >= 0) {
        start = i;
        len = query.length;
      }
    }
  } catch {
    return line; // an invalid JS regex — just show the line
  }
  if (start < 0) return line;
  return (
    <>
      {line.slice(0, start)}
      <span className="hl">{line.slice(start, start + len)}</span>
      {line.slice(start + len)}
    </>
  );
}

interface OutputSearchPanelProps {
  jobId: string;
}

export function OutputSearchPanel({ jobId }: OutputSearchPanelProps) {
  const [query, setQuery] = useState("");
  const [regex, setRegex] = useState(false);
  const [caseSensitive, setCaseSensitive] = useState(false);
  const [result, setResult] = useState<SearchResult | null>(null);
  const [searching, setSearching] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [presets, setPresets] = useState<SearchPresetInfo[]>([]);

  useEffect(() => {
    invoke<SearchPresetInfo[]>("get_search_presets")
      .then(setPresets)
      .catch(() => {
        /* presets are a nicety; a failure just hides the chips */
      });
  }, []);

  const run = useCallback(
    async (q: string, useRegex: boolean, cs: boolean) => {
      if (!q.trim()) {
        setResult(null);
        return;
      }
      setSearching(true);
      setError(null);
      try {
        const res = await invoke<SearchResult>("search_job_output", {
          id: jobId,
          opts: { query: q, regex: useRegex, case_sensitive: cs },
        });
        setResult(res);
      } catch (e) {
        setError(String(e));
        setResult(null);
      } finally {
        setSearching(false);
      }
    },
    [jobId],
  );

  // Clicking a preset chip fills the box, sets its flags, and searches at once —
  // the whole point is not having to remember ORCA's exact wording.
  const applyPreset = (p: SearchPresetInfo) => {
    setQuery(p.query);
    setRegex(p.regex);
    setCaseSensitive(p.case_sensitive);
    run(p.query, p.regex, p.case_sensitive);
  };

  const header = result
    ? result.truncated
      ? `${result.matches.length} of ${result.total} matches (showing first ${result.matches.length})`
      : `${result.total} ${result.total === 1 ? "match" : "matches"}`
    : null;

  return (
    <div className="output-search">
      <div className="row" style={{ gap: 8, flexWrap: "wrap" }}>
        <input
          className="input mono"
          style={{ flex: 1, minWidth: 200 }}
          placeholder="search in output…"
          value={query}
          onChange={(e) => setQuery(e.currentTarget.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") run(query, regex, caseSensitive);
          }}
          spellCheck={false}
        />
        <button
          className="btn btn-sm"
          onClick={() => run(query, regex, caseSensitive)}
          disabled={searching}
        >
          {searching ? "Searching…" : "Search"}
        </button>
        <label className="row search-opt" title="Regular expression">
          <input
            type="checkbox"
            checked={regex}
            onChange={(e) => setRegex(e.currentTarget.checked)}
          />
          regex
        </label>
        <label className="row search-opt" title="Case sensitive">
          <input
            type="checkbox"
            checked={caseSensitive}
            onChange={(e) => setCaseSensitive(e.currentTarget.checked)}
          />
          Aa
        </label>
      </div>

      {presets.length ? (
        <div className="search-presets">
          {presets.map((p) => (
            <button
              key={p.id}
              className="chip"
              title={p.description}
              onClick={() => applyPreset(p)}
            >
              {p.label}
            </button>
          ))}
        </div>
      ) : null}

      {error ? <div className="banner err">{error}</div> : null}

      {header ? <div className="search-count muted">{header}</div> : null}

      {result ? (
        result.matches.length ? (
          <div className="search-results">
            {result.matches.map((m) => (
              <div className="search-hit-block" key={m.line_no}>
                {m.context_before.map((c, i) => (
                  <div className="search-ctx" key={`b${i}`}>
                    <span className="ln">
                      {m.line_no - m.context_before.length + i}
                    </span>
                    {c}
                  </div>
                ))}
                <div className="search-hit">
                  <span className="ln">{m.line_no}</span>
                  {highlight(m.line, query, regex, caseSensitive)}
                </div>
                {m.context_after.map((c, i) => (
                  <div className="search-ctx" key={`a${i}`}>
                    <span className="ln">{m.line_no + 1 + i}</span>
                    {c}
                  </div>
                ))}
              </div>
            ))}
          </div>
        ) : (
          <div className="muted" style={{ fontSize: 12 }}>
            No matches
          </div>
        )
      ) : null}
    </div>
  );
}
