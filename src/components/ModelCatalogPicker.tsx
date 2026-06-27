import type { TFunction } from "i18next";
import { useEffect, useMemo, useState } from "react";
import type { ProviderModelCatalogItem } from "../types";

interface ModelCatalogPickerProps {
  id: string;
  value: string;
  onChange: (value: string) => void;
  catalog: ProviderModelCatalogItem[];
  t: TFunction;
  placeholder?: string;
  ariaLabel?: string;
  disabled?: boolean;
}

function modelOptionLabel(item: ProviderModelCatalogItem): string {
  return item.display_name === item.id
    ? item.id
    : `${item.display_name} (${item.id})`;
}

export default function ModelCatalogPicker({
  id,
  value,
  onChange,
  catalog,
  t,
  placeholder,
  ariaLabel,
  disabled = false,
}: ModelCatalogPickerProps) {
  const [open, setOpen] = useState(false);
  const [activeIndex, setActiveIndex] = useState(0);
  const [filterText, setFilterText] = useState("");
  const listboxId = `${id}-catalog-listbox`;
  const hintId = `${id}-catalog-hint`;
  const statusId = `${id}-catalog-status`;
  const descriptionIds = `${hintId} ${statusId}`;
  const query = filterText.trim().toLowerCase();
  const options = useMemo(() => {
    if (query.length === 0) return catalog;
    return catalog.filter((item) => {
      const haystack = `${item.id} ${item.display_name}`.toLowerCase();
      return haystack.includes(query);
    });
  }, [catalog, query]);
  const hasCatalog = catalog.length > 0;
  const expanded = open && hasCatalog && !disabled;
  const clampedActiveIndex =
    options.length > 0 ? Math.min(activeIndex, options.length - 1) : 0;
  const activeOption =
    expanded && options.length > 0 ? options[clampedActiveIndex] : undefined;
  const noResultsMessage = t("settings.modelPicker.noResults");
  const statusMessage =
    expanded && options.length === 0 ? noResultsMessage : "";
  const listboxLabel = ariaLabel
    ? `${ariaLabel} catalog options`
    : "Model catalog options";

  useEffect(() => {
    if (disabled) {
      setOpen(false);
      setFilterText("");
    }
  }, [disabled]);

  useEffect(() => {
    setActiveIndex((index) => {
      if (options.length === 0) return 0;
      return Math.min(index, options.length - 1);
    });
  }, [options.length]);

  const selectOption = (item: ProviderModelCatalogItem) => {
    if (disabled) return;
    onChange(item.id);
    setOpen(false);
    setActiveIndex(0);
    setFilterText("");
  };

  return (
    <div className="settings-model-picker">
      <input
        id={id}
        className="settings-input settings-model-picker__input"
        type="text"
        role="combobox"
        aria-autocomplete="list"
        aria-label={ariaLabel}
        aria-expanded={expanded}
        aria-controls={hasCatalog ? listboxId : undefined}
        aria-describedby={descriptionIds}
        aria-activedescendant={
          activeOption
            ? `${id}-catalog-option-${clampedActiveIndex}`
            : undefined
        }
        disabled={disabled}
        value={value}
        onChange={(e) => {
          onChange(e.target.value);
          setFilterText(e.target.value);
          setOpen(true);
          setActiveIndex(0);
        }}
        onFocus={() => {
          if (disabled) return;
          setFilterText("");
          setOpen(hasCatalog);
          setActiveIndex(0);
        }}
        onBlur={() =>
          window.setTimeout(() => {
            setOpen(false);
            setFilterText("");
          }, 100)
        }
        onKeyDown={(e) => {
          if (disabled || !hasCatalog) return;
          if (e.key === "ArrowDown") {
            e.preventDefault();
            setOpen(true);
            setActiveIndex((index) =>
              Math.min(index + 1, Math.max(options.length - 1, 0)),
            );
          } else if (e.key === "ArrowUp") {
            e.preventDefault();
            setOpen(true);
            setActiveIndex((index) => Math.max(index - 1, 0));
          } else if (e.key === "Home" && expanded && options.length > 0) {
            e.preventDefault();
            setActiveIndex(0);
          } else if (e.key === "End" && expanded && options.length > 0) {
            e.preventDefault();
            setActiveIndex(options.length - 1);
          } else if (e.key === "Enter" && expanded && activeOption) {
            e.preventDefault();
            selectOption(activeOption);
          } else if (e.key === "Escape") {
            setOpen(false);
          }
        }}
        placeholder={placeholder}
      />
      <span
        id={statusId}
        className="sr-only"
        role="status"
        aria-live="polite"
        aria-atomic="true"
      >
        {statusMessage}
      </span>
      {expanded && (
        <div
          id={listboxId}
          className="settings-model-picker__list"
          role="listbox"
          aria-label={listboxLabel}
          aria-describedby={descriptionIds}
        >
          {options.length === 0 ? (
            <div className="settings-model-picker__empty" aria-hidden="true">
              {noResultsMessage}
            </div>
          ) : (
            options.map((item, index) => (
              <div
                id={`${id}-catalog-option-${index}`}
                key={item.id}
                className={`settings-model-picker__option ${
                  index === clampedActiveIndex
                    ? "settings-model-picker__option--active"
                    : ""
                }`}
                role="option"
                aria-selected={item.id === value}
                tabIndex={-1}
                onMouseDown={(e) => {
                  e.preventDefault();
                  selectOption(item);
                }}
              >
                <span>{modelOptionLabel(item)}</span>
                {item.is_default && (
                  <span className="settings-model-picker__default">
                    {t("settings.modelPicker.default")}
                  </span>
                )}
              </div>
            ))
          )}
        </div>
      )}
      <p id={hintId} className="settings-model-picker__hint">
        {hasCatalog
          ? t("settings.modelPicker.customAllowed")
          : t("settings.modelPicker.emptyCatalog")}
      </p>
    </div>
  );
}
