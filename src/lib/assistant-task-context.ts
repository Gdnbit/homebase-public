import capabilityFormat from "../../control/capability-selection-format.json";
import workflowFormat from "../../control/workflow-context-format.json";
import {
  readCapabilityDraft,
  type CapabilityDraftView,
} from "./capability-context";
import { readWorkflowDraft, type WorkflowDraftView } from "./workflow-context";

export type AssistantTaskContext =
  | (CapabilityDraftView & { kind: "capability" })
  | (WorkflowDraftView & { kind: "workflow" });

export function readAssistantTaskContext(
  text: string,
): AssistantTaskContext | null {
  const hasCapability = text.includes(capabilityFormat.start),
    hasWorkflow = text.includes(workflowFormat.start);
  if (hasCapability === hasWorkflow) return null; // Plain input or mixed frames stay raw.
  if (hasCapability) {
    const capability = readCapabilityDraft(text);
    return capability ? { ...capability, kind: "capability" } : null;
  }
  const workflow = readWorkflowDraft(text);
  return workflow ? { ...workflow, kind: "workflow" } : null;
}
