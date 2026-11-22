export interface DocumentReference {
  document_id: string;
  extraction_id: string;
  source_name: string;
  source_sha256: string;
  page_from: number;
  page_to: number;
  text_from: number;
  text_to: number;
  content_sha256: string;
}
