/** A partial projection names an inclusive range of canonical frame positions. */
export function hasUnavailableProjectionBasis(value: unknown): boolean {
  if (
    typeof value !== "object" ||
    value === null ||
    !("first_sequence" in value) ||
    !("last_sequence" in value)
  )
    return false;
  const { first_sequence: first, last_sequence: last } = value;
  return (
    typeof first === "number" &&
    Number.isSafeInteger(first) &&
    first > 0 &&
    typeof last === "number" &&
    Number.isSafeInteger(last) &&
    last >= first
  );
}
