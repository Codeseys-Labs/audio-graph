/**
 * Button (ADR-0009 / ADR-0011).
 *
 * Shared, token-driven button base that replaces the ~10 bespoke per-component
 * button rules. Variants/sizes map to CSS in App.css (`.btn`, `.btn--<variant>`,
 * `.btn--<size>`). Supports an optional leading icon and a `loading` state that
 * disables the button and shows a spinner (W3.1 in-flight feedback).
 *
 * Focus ring is provided globally by `:focus-visible` (ADR-0009).
 */
import type { ButtonHTMLAttributes, ReactNode } from "react";
import Icon, { type IconName } from "./Icon";

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: "primary" | "secondary" | "ghost" | "danger";
  size?: "sm" | "md";
  /** Optional leading icon name. */
  icon?: IconName;
  /** Show a spinner and disable the button while an action is in flight. */
  loading?: boolean;
  children?: ReactNode;
}

export default function Button({
  variant = "secondary",
  size = "md",
  icon,
  loading = false,
  disabled,
  className,
  type = "button",
  children,
  ...rest
}: ButtonProps) {
  return (
    <button
      type={type}
      className={`btn btn--${variant} btn--${size}${loading ? " btn--loading" : ""}${className ? ` ${className}` : ""}`}
      disabled={disabled || loading}
      aria-busy={loading || undefined}
      {...rest}
    >
      {loading ? (
        <span className="btn__spinner" aria-hidden="true" />
      ) : (
        icon && <Icon name={icon} size={size === "sm" ? 14 : 16} />
      )}
      {children}
    </button>
  );
}
