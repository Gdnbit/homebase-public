/** Public comparison facts. These do not grant permission or guarantee a bill. */
export interface PriceInfo {
  route_id: string;
  state: "reported" | "partial" | "invalid";
  source_url: string;
  observed_at: string;
  reference_only: boolean;
  rates: {
    kind: "prompt" | "completion" | "request";
    amount: string;
    unit: "usd_per_token" | "usd_per_request";
    per_million_tokens: string | null;
  }[];
  unmodeled_dimensions: number;
  missing_dimensions: number;
  invalid_dimensions: number;
  conditional: boolean;
}
export interface EvidenceInfo {
  state: "registry_recorded" | "records_checked";
  source_commit: string;
  package_version: string;
  manifest_sha256: string;
  verification_sha256: string;
  registered_at: string;
  checked_at: string | null;
  code_integrity_checked: boolean;
}
