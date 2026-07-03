import { fireEvent, render, screen } from "@testing-library/react";
import type { TFunction } from "i18next";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import type { ProviderModelCatalogItem } from "../types";
import ModelCatalogPicker from "./ModelCatalogPicker";

const t = ((key: string) => {
  const translations: Record<string, string> = {
    "settings.modelPicker.default": "Default",
    "settings.modelPicker.customAllowed":
      "Search the catalog or type a custom model id.",
    "settings.modelPicker.emptyCatalog":
      "No catalog loaded. Type a custom model id.",
    "settings.modelPicker.noResults":
      "No matching models. Press Tab to keep the custom value.",
  };
  return translations[key] ?? key;
}) as TFunction;

const catalog: ProviderModelCatalogItem[] = [
  {
    id: "openai/gpt-5.2",
    display_name: "OpenAI: GPT-5.2",
    is_default: true,
  },
  {
    id: "anthropic/claude-sonnet-4.5",
    display_name: "Anthropic: Claude Sonnet 4.5",
    is_default: false,
  },
  {
    id: "google/gemini-3-pro",
    display_name: "Google: Gemini 3 Pro",
    is_default: false,
  },
];

function renderPicker(
  props: Partial<{
    value: string;
    catalog: ProviderModelCatalogItem[];
    onChange: (value: string) => void;
  }> = {},
) {
  const onChange = props.onChange ?? vi.fn();

  function Harness() {
    const [value, setValue] = useState(props.value ?? "");
    return (
      <ModelCatalogPicker
        id="model-picker"
        value={value}
        onChange={(nextValue) => {
          setValue(nextValue);
          onChange(nextValue);
        }}
        catalog={props.catalog ?? catalog}
        t={t}
        ariaLabel="Model"
      />
    );
  }

  render(<Harness />);
  return { onChange };
}

describe("ModelCatalogPicker", () => {
  it("connects the combobox and listbox to stable hint and status ids", () => {
    renderPicker();

    const picker = screen.getByRole("combobox", { name: /model/i });
    expect(picker).toHaveAttribute(
      "aria-describedby",
      "model-picker-catalog-hint model-picker-catalog-status",
    );
    expect(picker).toHaveAccessibleDescription(
      /Search the catalog or type a custom model id\./i,
    );
    expect(screen.getByText(/Search the catalog/i)).toHaveAttribute(
      "id",
      "model-picker-catalog-hint",
    );
    expect(screen.getByRole("status")).toHaveAttribute(
      "id",
      "model-picker-catalog-status",
    );

    fireEvent.focus(picker);

    const listbox = screen.getByRole("listbox", {
      name: "Model catalog options",
    });
    expect(listbox).toHaveAttribute("id", "model-picker-catalog-listbox");
    expect(listbox).toHaveAttribute(
      "aria-describedby",
      "model-picker-catalog-hint model-picker-catalog-status",
    );
  });

  it("moves with ArrowDown and ArrowUp, then selects the active option with Enter", () => {
    renderPicker();

    const picker = screen.getByRole("combobox", { name: /model/i });
    fireEvent.focus(picker);
    expect(picker).toHaveAttribute(
      "aria-activedescendant",
      "model-picker-catalog-option-0",
    );

    fireEvent.keyDown(picker, { key: "ArrowDown" });
    fireEvent.keyDown(picker, { key: "ArrowDown" });
    expect(picker).toHaveAttribute(
      "aria-activedescendant",
      "model-picker-catalog-option-2",
    );

    fireEvent.keyDown(picker, { key: "ArrowUp" });
    expect(picker).toHaveAttribute(
      "aria-activedescendant",
      "model-picker-catalog-option-1",
    );

    fireEvent.keyDown(picker, { key: "Enter" });
    expect(picker).toHaveValue("anthropic/claude-sonnet-4.5");
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
  });

  it("moves to the first and last catalog options with Home and End", () => {
    renderPicker();

    const picker = screen.getByRole("combobox", { name: /model/i });
    fireEvent.focus(picker);

    fireEvent.keyDown(picker, { key: "End" });
    expect(picker).toHaveAttribute(
      "aria-activedescendant",
      "model-picker-catalog-option-2",
    );

    fireEvent.keyDown(picker, { key: "Home" });
    expect(picker).toHaveAttribute(
      "aria-activedescendant",
      "model-picker-catalog-option-0",
    );

    fireEvent.keyDown(picker, { key: "Enter" });
    expect(picker).toHaveValue("openai/gpt-5.2");
  });

  it("closes the catalog list with Escape", () => {
    renderPicker();

    const picker = screen.getByRole("combobox", { name: /model/i });
    fireEvent.focus(picker);
    expect(screen.getByRole("listbox")).toBeInTheDocument();

    fireEvent.keyDown(picker, { key: "Escape" });
    expect(picker).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
  });

  it("announces no results without exposing a fake selectable option", () => {
    renderPicker();

    const picker = screen.getByRole("combobox", { name: /model/i });
    fireEvent.change(picker, { target: { value: "bespoke/model-id" } });

    const noResultsText = screen
      .getAllByText(/No matching models\. Press Tab to keep the custom value/i)
      .find((element) =>
        element.classList.contains("settings-model-picker__empty"),
      );
    if (!noResultsText) {
      throw new Error("Expected visible no-results text");
    }
    expect(noResultsText).toBeInTheDocument();
    expect(noResultsText).toHaveAttribute("aria-hidden", "true");
    expect(screen.getByRole("status")).toHaveTextContent(
      "No matching models. Press Tab to keep the custom value.",
    );
    expect(
      screen.queryByRole("option", { name: /no matching models/i }),
    ).not.toBeInTheDocument();
    expect(picker).not.toHaveAttribute("aria-activedescendant");
    expect(picker).toHaveValue("bespoke/model-id");

    fireEvent.keyDown(picker, { key: "Enter" });
    expect(picker).toHaveValue("bespoke/model-id");

    fireEvent.keyDown(picker, { key: "Escape" });
    expect(picker).toHaveValue("bespoke/model-id");
  });

  it("clamps the active catalog option when the catalog shrinks", () => {
    function Harness({ items }: { items: ProviderModelCatalogItem[] }) {
      const [value, setValue] = useState("");
      return (
        <ModelCatalogPicker
          id="model-picker"
          value={value}
          onChange={setValue}
          catalog={items}
          t={t}
          ariaLabel="Model"
        />
      );
    }

    const { rerender } = render(<Harness items={catalog} />);
    const picker = screen.getByRole("combobox", { name: /model/i });
    fireEvent.focus(picker);
    fireEvent.keyDown(picker, { key: "End" });
    expect(picker).toHaveAttribute(
      "aria-activedescendant",
      "model-picker-catalog-option-2",
    );

    rerender(<Harness items={[catalog[0]]} />);

    expect(picker).toHaveAttribute(
      "aria-activedescendant",
      "model-picker-catalog-option-0",
    );
    fireEvent.keyDown(picker, { key: "Enter" });
    expect(picker).toHaveValue("openai/gpt-5.2");
  });

  it("keeps a custom typed value when the catalog is empty", () => {
    renderPicker({ catalog: [] });

    const picker = screen.getByRole("combobox", { name: /model/i });
    fireEvent.change(picker, { target: { value: "custom/no-catalog" } });

    expect(picker).toHaveValue("custom/no-catalog");
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    expect(
      screen.getByText(/No catalog loaded\. Type a custom model id/i),
    ).toBeInTheDocument();
  });

  it("snaps a recognized alias to its canonical id on blur via normalizeOnBlur", () => {
    // Mirrors the Deepgram bare `flux` -> `flux-general-en` snap (FIX-1
    // frontend guard). The picker stays provider-agnostic; the mapping is
    // supplied by the caller.
    const onChange = vi.fn();
    const normalizeOnBlur = (value: string) =>
      value.trim().toLowerCase() === "flux" ? "flux-general-en" : value;

    function Harness() {
      const [value, setValue] = useState("");
      return (
        <ModelCatalogPicker
          id="model-picker"
          value={value}
          onChange={(next) => {
            setValue(next);
            onChange(next);
          }}
          catalog={catalog}
          t={t}
          ariaLabel="Model"
          normalizeOnBlur={normalizeOnBlur}
        />
      );
    }
    render(<Harness />);

    const picker = screen.getByRole("combobox", { name: /model/i });
    fireEvent.change(picker, { target: { value: "flux" } });
    expect(picker).toHaveValue("flux");

    fireEvent.blur(picker);
    expect(onChange).toHaveBeenLastCalledWith("flux-general-en");
    expect(picker).toHaveValue("flux-general-en");
  });

  it("leaves an already-canonical value untouched on blur", () => {
    const onChange = vi.fn();
    const normalizeOnBlur = (value: string) =>
      value.trim().toLowerCase() === "flux" ? "flux-general-en" : value;

    function Harness() {
      const [value, setValue] = useState("flux-general-en");
      return (
        <ModelCatalogPicker
          id="model-picker"
          value={value}
          onChange={(next) => {
            setValue(next);
            onChange(next);
          }}
          catalog={catalog}
          t={t}
          ariaLabel="Model"
          normalizeOnBlur={normalizeOnBlur}
        />
      );
    }
    render(<Harness />);

    const picker = screen.getByRole("combobox", { name: /model/i });
    fireEvent.blur(picker);
    // No normalization needed -> onChange must NOT fire (value unchanged).
    expect(onChange).not.toHaveBeenCalled();
    expect(picker).toHaveValue("flux-general-en");
  });
});
