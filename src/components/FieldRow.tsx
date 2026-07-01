/**
 * FieldRow — the settings-field / __label / control triad primitive.
 *
 * Settings panels (AsrProviderSettings, AudioSettings, CredentialsManager, …)
 * repeat the same markup for a labelled control:
 *
 *   <div className="settings-field">
 *     <label className="settings-field__label" htmlFor={id}>{label}</label>
 *     {control}            // <select> / <input> / <ModelCatalogPicker> / …
 *     {hint && <p className="settings-hint">{hint}</p>}
 *   </div>
 *
 * FieldRow encapsulates that wrapper + label + optional hint while leaving the
 * control to the caller as children. The `htmlFor`/`id` association is
 * preserved verbatim so `getByLabelText` queries keep working, and the class
 * names are unchanged so existing styles/selectors are untouched.
 */
import type { ReactNode } from "react";

export interface FieldRowProps {
  /** The `id` of the control this label points at (label `htmlFor`). */
  htmlFor: string;
  /** The field label text/content. */
  label: ReactNode;
  /** The control element(s) — select, input, picker, etc. */
  children: ReactNode;
  /** Optional hint rendered below the control as `.settings-hint`. */
  hint?: ReactNode;
  /** Optional extra class on the wrapping `.settings-field`. */
  className?: string;
}

export default function FieldRow({
  htmlFor,
  label,
  children,
  hint,
  className,
}: FieldRowProps) {
  return (
    <div className={`settings-field${className ? ` ${className}` : ""}`}>
      <label className="settings-field__label" htmlFor={htmlFor}>
        {label}
      </label>
      {children}
      {hint != null && <p className="settings-hint">{hint}</p>}
    </div>
  );
}
