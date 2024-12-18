import React from 'react';

interface ProgressBarProps {
  message: string;
  position: number;
  length: number;
}

function formatBytes(bytes: number): string {
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
  let value = bytes;
  let unitIndex = 0;
  
  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024;
    unitIndex++;
  }
  
  return `${value.toFixed(2).replace(/\.?0+$/, '')} ${units[unitIndex]}`;
}

export const ProgressBar: React.FC<ProgressBarProps> = ({ message, position, length }) => {
  const percentage = length > 0 ? (position / length) * 100 : 0;
  
  return (
    <div className="w-full bg-irohGray-100 p-4">
      <div className="space-y-2">
        <div className="w-full bg-irohGray-200 h-1 rounded-full overflow-hidden">
          <div 
            className="bg-irohPurple-500 h-full transition-none"
            style={{ width: `${percentage}%` }}
          />
        </div>
        <div className="flex justify-between text-xs font-spaceMono text-irohGray-500">
          <span>{message}</span>
          <span>{`${formatBytes(position)} / ${formatBytes(length)}`}</span>
        </div>
      </div>
    </div>
  );
};