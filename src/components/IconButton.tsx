/**
 * IconButton (ADR-0010).
 *
 * Accessible icon-only button. The `label` prop is required and becomes both
 * the `aria-label` and the hover `title`, so every icon button has a usable
 * accessible name (fixing the emoji buttons that shipped without one). Focus
 * ring comes from the global `:focus-visible` rule (ADR-0009).
 *
 * Variants map to token-driven CSS in App.css (`.icon-btn--<variant>`).
 */
import type { ButtonHTMLAttributes } from "react";
import Icon, { type IconName } from "./Icon";

export interface IconButtonProps
  extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, "aria-label"> {
  icon: IconName;
  /** Required accessible name (used for aria-label + title). */
  label: string;
  size?: number;
  /** Visual treatment. `default` is a subtle ghost button. */
  variant?: "default" | "ghost" | "danger" | "active";
}

export default function IconButton({
  icon,
  label,
  size = 16,
  variant = "default",
  className,
  type = "button",
  ...rest
}: IconButtonProps) {
  return (
    <button
      type={type}
      className={`icon-btn icon-btn--${variant}${className ? ` ${className}` : ""}`}
      aria-label={label}
      title={label}
      {...rest}
    >
      <Icon name={icon} size={size} />
    </button>
  );
}
