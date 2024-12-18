import { useState } from 'react';

interface ConnectingScreenProps {
  onBack: () => void;
  onScanClick: () => void;
  onConnect: (nodeId: string) => void;
}

export function ConnectingScreen({ onBack, onScanClick, onConnect }: ConnectingScreenProps) {
  const [nodeId, setNodeId] = useState('');

  return (
    <div className="w-full">
      <button 
        onClick={onBack}
        className="mb-8 text-irohGray-500 hover:text-irohGray-800 transition flex items-center gap-2"
      >
        ← Back
      </button>

      <div className="flex flex-col space-y-6">
        <div className="flex flex-col space-y-2">
          <label htmlFor="nodeId" className="text-sm text-irohGray-600">
            Copy in NodeId
          </label>
          <input
            id="nodeId"
            type="text"
            value={nodeId}
            onChange={(e) => setNodeId(e.target.value)}
            className="p-3 border border-irohGray-200 focus:border-irohPurple-500 focus:outline-none"
            placeholder="Copy in the NodeId to connect to..."
          />
        </div>

        <button
          onClick={() => onConnect(nodeId)}
          className="w-full p-3 px-4 transition bg-irohGray-800 text-irohPurple-500 uppercase hover:bg-irohGray-700 hover:text-gray-200 font-medium"
        >
          Connect
        </button>

        <button
          onClick={onScanClick}
          className="w-full p-3 px-4 transition bg-white text-irohGray-800 uppercase hover:bg-irohGray-100 border border-irohGray-200 font-medium"
        >
          Scan QR Code Instead
        </button>
      </div>
    </div>
  );
} 