import { isDeepStrictEqual } from "node:util";

// Offline JSON Schema subset emitted by the reviewed contract generator. Never executes a command.
export function validateSchema(
  schema,
  value,
  definitions = {},
  location = "$",
) {
  const errors = [];
  const bad = (rule) => errors.push({ path: location, rule });
  if (schema.$ref) {
    const ref = schema.$ref.split("/").at(-1);
    if (!definitions[ref])
      return [{ path: location, rule: "schema_reference_missing" }];
    return validateSchema(definitions[ref], value, definitions, location);
  }
  if (schema.anyOf || schema.oneOf) {
    const choices = schema.anyOf || schema.oneOf;
    const valid = choices.filter(
      (s) => validateSchema(s, value, definitions, location).length === 0,
    ).length;
    if (schema.oneOf ? valid !== 1 : valid === 0)
      bad(schema.oneOf ? "oneOf" : "anyOf");
    return errors;
  }
  if (
    Object.hasOwn(schema, "const") &&
    JSON.stringify(value) !== JSON.stringify(schema.const)
  )
    bad("const");
  if (schema.enum && !schema.enum.includes(value)) bad("enum");
  const actual =
    value === null ? "null" : Array.isArray(value) ? "array" : typeof value;
  if (
    schema.type &&
    (schema.type === "integer"
      ? !Number.isSafeInteger(value)
      : schema.type === "number"
        ? typeof value !== "number" || !Number.isFinite(value)
        : actual !== schema.type)
  ) {
    bad(`type:${schema.type}`);
    return errors;
  }
  if (typeof value === "number") {
    if (schema.minimum != null && value < schema.minimum) bad("minimum");
    if (schema.maximum != null && value > schema.maximum) bad("maximum");
  }
  if (typeof value === "string") {
    if (schema.maxLength != null && [...value].length > schema.maxLength)
      bad("maxLength");
    if (schema.minLength != null && [...value].length < schema.minLength)
      bad("minLength");
    if (schema.pattern && !new RegExp(schema.pattern).test(value))
      bad("pattern");
  }
  if (actual === "array") {
    if (schema.minItems != null && value.length < schema.minItems)
      bad("minItems");
    if (schema.maxItems != null && value.length > schema.maxItems) {
      bad("maxItems");
      return errors;
    }
    if (
      schema.uniqueItems &&
      value.some((item, i) =>
        value
          .slice(0, i)
          .some(
            (previous) =>
              item === previous || isDeepStrictEqual(item, previous),
          ),
      )
    )
      bad("uniqueItems");
    if (schema.items)
      value.forEach((v, i) =>
        errors.push(
          ...validateSchema(schema.items, v, definitions, `${location}[${i}]`),
        ),
      );
  }
  if (actual === "object") {
    for (const key of schema.required || [])
      if (!Object.hasOwn(value, key))
        errors.push({ path: `${location}.${key}`, rule: "required" });
    for (const [key, item] of Object.entries(value)) {
      const property = schema.properties?.[key];
      if (property)
        errors.push(
          ...validateSchema(property, item, definitions, `${location}.${key}`),
        );
      else if (schema.additionalProperties === false)
        errors.push({
          path: `${location}.${key}`,
          rule: "additionalProperties",
        });
      else if (
        schema.additionalProperties &&
        typeof schema.additionalProperties === "object"
      )
        errors.push(
          ...validateSchema(
            schema.additionalProperties,
            item,
            definitions,
            `${location}.${key}`,
          ),
        );
    }
  }
  return errors;
}
export function canonicalArguments(command, value) {
  if (!value || typeof value !== "object" || Array.isArray(value)) return value;
  const result = { ...value };
  for (const p of command.parameters)
    if (p.name !== p.rust_name && Object.hasOwn(result, p.rust_name)) {
      if (Object.hasOwn(result, p.name))
        throw Error("duplicate_argument_alias");
      result[p.name] = result[p.rust_name];
      delete result[p.rust_name];
    }
  return result;
}
export function reachableDefinitions(schema, all) {
  const result = {};
  function visit(value) {
    if (!value || typeof value !== "object") return;
    if (value.$ref) {
      const key = value.$ref.split("/").at(-1);
      if (!all[key]) throw Error("missing_schema_definition");
      if (!result[key]) {
        result[key] = all[key];
        visit(all[key]);
      }
    }
    for (const child of Object.values(value))
      if (typeof child === "object") visit(child);
  }
  visit(schema);
  return result;
}
