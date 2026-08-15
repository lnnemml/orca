import type { OrcaOption } from "./orca-options";

/**
 * A labelled `<select>` over a list of {@link OrcaOption}s. Extracted from
 * `InputBuilderForm` so both the form and the reusable `<MethodPicker>` render the
 * same control (identical markup, so the New Job DOM is unchanged by the extraction).
 */
export function OptionSelect({
  label,
  value,
  options,
  onChange,
  disabled,
  title,
}: {
  label: string;
  value: string;
  options: OrcaOption[];
  onChange: (v: string) => void;
  disabled?: boolean;
  /** Tooltip on the control — e.g. why it is disabled. */
  title?: string;
}) {
  return (
    <div className="field">
      <label className="label">{label}</label>
      <select
        className="input select"
        value={value}
        onChange={(e) => onChange(e.currentTarget.value)}
        disabled={disabled}
        title={title}
      >
        {options.map((o) => (
          <option key={o.keyword || "__none__"} value={o.keyword}>
            {o.label}
          </option>
        ))}
      </select>
    </div>
  );
}
