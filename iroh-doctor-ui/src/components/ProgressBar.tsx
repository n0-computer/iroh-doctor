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
  
  // Format with up to 2 decimal places, but remove trailing zeros
  return `${value.toFixed(2).replace(/\.?0+$/, '')} ${units[unitIndex]}`;
}

export class ProgressBarWrapper {
  private setMessage: (msg: string) => void;
  private setPosition: (pos: number) => void;
  private setLength: (len: number) => void;

  constructor(
    setMessage: (msg: string) => void,
    setPosition: (pos: number) => void,
    setLength: (len: number) => void
  ) {
    this.setMessage = setMessage;
    this.setPosition = setPosition;
    this.setLength = setLength;
  }

  // Implements ProgressBarExt trait methods
  set_message(msg: string) {
    this.setMessage(msg);
  }

  set_position(pos: number) {
    this.setPosition(pos);
  }

  set_length(len: number) {
    this.setLength(len);
  }
}

export const ProgressBar: React.FC<ProgressBarProps> = ({ message, position, length }) => {
  const percentage = length > 0 ? (position / length) * 100 : 0;
  
  return (
    <div className="w-full">
      <div className="flex justify-between mb-1">
        <span className="text-sm font-medium">{message}</span>
        <span className="text-sm font-medium">
          {`${formatBytes(position)} / ${formatBytes(length)}`}
        </span>
      </div>
      <div className="w-full bg-gray-200 rounded-full h-2.5">
        <div 
          className="bg-cyan-500 h-2.5 rounded-full" 
          style={{ width: `${percentage}%` }}
        />
      </div>
    </div>
  );
};