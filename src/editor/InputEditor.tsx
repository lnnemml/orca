import Editor, { type Monaco } from "@monaco-editor/react";

// Side-effect: point Monaco at the bundled package + worker (must run before mount).
import "./monaco-setup";
import {
  orcaLanguageConfiguration,
  orcaLanguageId,
  orcaMonarchTokens,
} from "./orca-language";

// Register the ORCA language exactly once per Monaco instance. `beforeMount`
// fires on the first editor mount — the only point where `monaco` is available.
let registered = false;
function registerOrcaLanguage(monaco: Monaco) {
  if (registered) return;
  registered = true;
  monaco.languages.register({ id: orcaLanguageId });
  monaco.languages.setMonarchTokensProvider(orcaLanguageId, orcaMonarchTokens);
  monaco.languages.setLanguageConfiguration(
    orcaLanguageId,
    orcaLanguageConfiguration,
  );
}

interface InputEditorProps {
  value: string;
  onChange: (value: string) => void;
}

/** Full-height Monaco editor wired to the ORCA `.inp` grammar (vs-dark). */
export function InputEditor({ value, onChange }: InputEditorProps) {
  return (
    <Editor
      language={orcaLanguageId}
      theme="vs-dark"
      value={value}
      onChange={(next) => onChange(next ?? "")}
      beforeMount={registerOrcaLanguage}
      options={{
        fontFamily: "ui-monospace, 'SF Mono', Menlo, Consolas, monospace",
        fontSize: 13,
        minimap: { enabled: false },
        scrollBeyondLastLine: false,
        automaticLayout: true,
        tabSize: 2,
        renderWhitespace: "none",
        smoothScrolling: true,
      }}
    />
  );
}
