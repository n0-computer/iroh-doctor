import { invoke } from '@tauri-apps/api/core';

export interface ProgressState {
  position: number;
  length: number;
  message: string;
}

export async function startAcceptingConnections(): Promise<string> {
  return invoke('start_accepting_connections');
}

export async function connectToNode(nodeId: string): Promise<void> {
  return invoke('connect_to_node', { nodeId });
}

export async function getProgressState(): Promise<ProgressState> {
  return invoke('get_progress_state');
}
