import Editor, { type Monaco } from "@monaco-editor/react";
import type { editor as MonacoEditor } from "monaco-editor";

// Side-effect: point Monaco at the bundled package + worker (must run before mount).
import "./monaco-setup";
import {
  orcaLanguageConfiguration,
  orcaLanguageId,
  orcaMonarchTokens,
} from "./orca-language";
import { registerSelectionLookup } from "./selection-panel";
import { orcaEditorOptions } from "./editor-options";
import { ManualDrawer } from "../manual/ManualDrawer";

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

/** Full-height Monaco editor wired to the ORCA `.inp` grammar (vs-dark). Manual help is
 *  triggered by a SELECTION (unit 4.13, not hover): `registerSelectionLookup` (on mount,
 *  where the editor instance is available) shows a floating panel over a settled selection.
 *  The `ManualDrawer` is a fixed-position overlay (opened by the panel's "Open" action), so
 *  it does not affect the editor's layout and the author stays in the editor. */
export function InputEditor({ value, onChange }: InputEditorProps) {
  return (
    <>
      <Editor
        language={orcaLanguageId}
        theme="vs-dark"
        value={value}
        onChange={(next) => onChange(next ?? "")}
        beforeMount={registerOrcaLanguage}
        onMount={(editor, monaco) =>
          registerSelectionLookup(monaco, editor as MonacoEditor.ICodeEditor)
        }
        options={orcaEditorOptions}
      />
      <ManualDrawer />
    </>
  );
}
