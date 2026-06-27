import type { TFunction } from "i18next";
import { useEffect, useRef, useState } from "react";
import Icon from "./Icon";

interface SecretCredentialControlProps {
  id: string;
  label: string;
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  saved: boolean;
  t: TFunction;
  disabled?: boolean;
  savedHint?: string;
  missingHint?: string;
  draftHint?: string;
  clearLabel?: string;
  onClear?: () => void;
}

interface AwsCredentialControlProps {
  accessKeyId: string;
  secretKeyId: string;
  sessionTokenId: string;
  accessKey: string;
  secretKey: string;
  sessionToken: string;
  onAccessKeyChange: (value: string) => void;
  onSecretKeyChange: (value: string) => void;
  onSessionTokenChange: (value: string) => void;
  saved: boolean;
  sessionTokenSaved: boolean;
  t: TFunction;
  disabled?: boolean;
  onClear?: () => void;
}

export default function SecretCredentialControl({
  id,
  label,
  value,
  onChange,
  placeholder,
  saved,
  t,
  disabled = false,
  savedHint,
  missingHint,
  draftHint,
  clearLabel,
  onClear,
}: SecretCredentialControlProps) {
  const hasDraft = value.trim().length > 0;
  const [editing, setEditing] = useState(hasDraft);
  const inputRef = useRef<HTMLInputElement | null>(null);
  const showInput = editing || hasDraft;
  const status = hasDraft ? "draft" : saved ? "saved" : "missing";
  const statusId = `${id}-credential-status`;
  const hintId = `${id}-credential-hint`;
  const describedBy = `${statusId} ${hintId}`;
  const primaryActionLabel = saved
    ? t("settings.credentialControl.replace")
    : t("settings.credentialControl.add");
  const cancelLabel = t("settings.credentialControl.cancel");
  const clearActionLabel = clearLabel ?? t("settings.credentialControl.clear");

  useEffect(() => {
    if (!showInput) return;
    inputRef.current?.focus();
  }, [showInput]);

  const statusLabel = t(`settings.credentialControl.status.${status}`);
  const hint =
    hasDraft && draftHint
      ? draftHint
      : hasDraft
        ? t("settings.credentialControl.draftHint")
        : saved
          ? (savedHint ?? t("settings.credentialControl.savedHint"))
          : (missingHint ?? t("settings.credentialControl.missingHint"));

  return (
    <div className="settings-field settings-credential-control">
      <div className="settings-credential-control__summary">
        <div className="settings-credential-control__copy">
          <span
            id={statusId}
            className={`settings-credential-control__badge settings-credential-control__badge--${status}`}
          >
            {statusLabel}
          </span>
          <p id={hintId} className="settings-hint">
            {hint}
          </p>
        </div>
        <div className="settings-credential-control__actions">
          {!showInput && (
            <button
              id={id}
              type="button"
              className="settings-btn settings-btn--secondary"
              onClick={() => setEditing(true)}
              disabled={disabled}
              aria-label={`${primaryActionLabel}: ${label}`}
              aria-describedby={describedBy}
            >
              <Icon name={saved ? "refresh" : "check"} size={14} />
              {primaryActionLabel}
            </button>
          )}
          {showInput && !hasDraft && (
            <button
              type="button"
              className="settings-btn settings-btn--secondary"
              onClick={() => setEditing(false)}
              disabled={disabled}
              aria-label={`${cancelLabel}: ${label}`}
              aria-describedby={describedBy}
            >
              <Icon name="close" size={14} />
              {cancelLabel}
            </button>
          )}
          {(saved || hasDraft) && onClear && (
            <button
              type="button"
              className="settings-btn settings-btn--danger"
              onClick={onClear}
              disabled={disabled}
              aria-label={`${clearActionLabel}: ${label}`}
              aria-describedby={describedBy}
            >
              <Icon name="trash" size={14} />
              {clearActionLabel}
            </button>
          )}
        </div>
      </div>
      {showInput && (
        <div className="settings-credential-control__input">
          <label className="settings-field__label" htmlFor={id}>
            {label}
          </label>
          <input
            id={id}
            ref={inputRef}
            className="settings-input"
            type="password"
            value={value}
            onChange={(e) => onChange(e.target.value)}
            placeholder={placeholder}
            autoComplete="off"
            disabled={disabled}
            aria-describedby={describedBy}
          />
        </div>
      )}
    </div>
  );
}

export function AwsCredentialControl({
  accessKeyId,
  secretKeyId,
  sessionTokenId,
  accessKey,
  secretKey,
  sessionToken,
  onAccessKeyChange,
  onSecretKeyChange,
  onSessionTokenChange,
  saved,
  sessionTokenSaved,
  t,
  disabled = false,
  onClear,
}: AwsCredentialControlProps) {
  const hasDraft =
    accessKey.trim().length > 0 ||
    secretKey.trim().length > 0 ||
    sessionToken.trim().length > 0;
  const [editing, setEditing] = useState(hasDraft);
  const accessKeyRef = useRef<HTMLInputElement | null>(null);
  const showInput = editing || hasDraft;
  const status = hasDraft ? "draft" : saved ? "saved" : "missing";
  const statusId = `${accessKeyId}-credential-status`;
  const hintId = `${accessKeyId}-credential-hint`;
  const describedBy = `${statusId} ${hintId}`;
  const awsCredentialLabel = t("settings.credentialConfirm.awsKeysLabel");
  const primaryActionLabel = saved
    ? t("settings.credentialControl.replaceAws")
    : t("settings.credentialControl.addAws");
  const cancelLabel = t("settings.credentialControl.cancel");
  const clearActionLabel = t("settings.buttons.clearSavedAwsKeys");

  useEffect(() => {
    if (!showInput) return;
    accessKeyRef.current?.focus();
  }, [showInput]);

  const hint = hasDraft
    ? t("settings.credentialControl.awsDraftHint")
    : saved
      ? sessionTokenSaved
        ? t("settings.credentialControl.awsSavedWithSessionHint")
        : t("settings.credentialControl.awsSavedHint")
      : t("settings.credentialControl.awsMissingHint");

  return (
    <div className="settings-field settings-credential-control">
      <div className="settings-credential-control__summary">
        <div className="settings-credential-control__copy">
          <span
            id={statusId}
            className={`settings-credential-control__badge settings-credential-control__badge--${status}`}
          >
            {t(`settings.credentialControl.status.${status}`)}
          </span>
          <p id={hintId} className="settings-hint">
            {hint}
          </p>
        </div>
        <div className="settings-credential-control__actions">
          {!showInput && (
            <button
              id={accessKeyId}
              type="button"
              className="settings-btn settings-btn--secondary"
              onClick={() => setEditing(true)}
              disabled={disabled}
              aria-label={`${primaryActionLabel}: ${awsCredentialLabel}`}
              aria-describedby={describedBy}
            >
              <Icon name={saved ? "refresh" : "check"} size={14} />
              {primaryActionLabel}
            </button>
          )}
          {showInput && !hasDraft && (
            <button
              type="button"
              className="settings-btn settings-btn--secondary"
              onClick={() => setEditing(false)}
              disabled={disabled}
              aria-label={`${cancelLabel}: ${awsCredentialLabel}`}
              aria-describedby={describedBy}
            >
              <Icon name="close" size={14} />
              {cancelLabel}
            </button>
          )}
          {saved && onClear && (
            <button
              type="button"
              className="settings-btn settings-btn--danger"
              onClick={onClear}
              disabled={disabled}
              aria-label={`${clearActionLabel}: ${awsCredentialLabel}`}
              aria-describedby={describedBy}
            >
              <Icon name="trash" size={14} />
              {clearActionLabel}
            </button>
          )}
        </div>
      </div>
      {showInput && (
        <div className="settings-credential-control__input settings-credential-control__input-grid">
          <div className="settings-field">
            <label className="settings-field__label" htmlFor={accessKeyId}>
              {t("settings.fields.accessKeyId")}
            </label>
            <input
              id={accessKeyId}
              ref={accessKeyRef}
              className="settings-input"
              type="password"
              value={accessKey}
              onChange={(e) => onAccessKeyChange(e.target.value)}
              placeholder="AKIA..."
              autoComplete="off"
              disabled={disabled}
              aria-describedby={describedBy}
            />
          </div>
          <div className="settings-field">
            <label className="settings-field__label" htmlFor={secretKeyId}>
              {t("settings.fields.secretAccessKey")}
            </label>
            <input
              id={secretKeyId}
              className="settings-input"
              type="password"
              value={secretKey}
              onChange={(e) => onSecretKeyChange(e.target.value)}
              placeholder="wJalr..."
              autoComplete="off"
              disabled={disabled}
              aria-describedby={describedBy}
            />
          </div>
          <div className="settings-field">
            <label className="settings-field__label" htmlFor={sessionTokenId}>
              {t("settings.fields.sessionTokenOptional")}
            </label>
            <input
              id={sessionTokenId}
              className="settings-input"
              type="password"
              value={sessionToken}
              onChange={(e) => onSessionTokenChange(e.target.value)}
              placeholder={t("settings.placeholders.sessionTokenHint")}
              autoComplete="off"
              disabled={disabled}
              aria-describedby={describedBy}
            />
          </div>
        </div>
      )}
    </div>
  );
}
