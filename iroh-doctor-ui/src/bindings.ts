import { invoke } from '@tauri-apps/api/core';

export async function startAcceptingConnections(): Promise<string> {
  return invoke('start_accepting_connections');
}

export async function connectToNode(nodeId: string): Promise<void> {
  return invoke('connect_to_node', { nodeId });
}
