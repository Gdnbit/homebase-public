import { createContext, useContext } from "react";

// Retained editors keep their draft and original task listener, while optional
// presentation reads and global input handlers follow actual route visibility.
export const WorkflowVisibility = createContext(true);
export const useWorkflowVisibility = () => useContext(WorkflowVisibility);
