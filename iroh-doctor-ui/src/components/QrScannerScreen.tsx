import { useEffect } from 'react';
import * as BarcodeScanner from '@tauri-apps/plugin-barcode-scanner';

interface QrScannerScreenProps {
  onBack: () => void;
  onScan: (nodeId: string) => void;
}

export function QrScannerScreen({ onBack, onScan }: QrScannerScreenProps) {
  useEffect(() => {
    const startScanning = async () => {
      try {
        const result = await BarcodeScanner.requestPermissions();
        if (result === 'granted') {
          const scan = await BarcodeScanner.scan();
          if (scan.content) {
            onScan(scan.content);
          }
        }
      } catch (err) {
        console.error('Scanning failed:', err);
      }
    };

    startScanning();
    
    return () => {
      BarcodeScanner.cancel();
    };
  }, [onScan]);

  return (
    <div className="w-full h-full relative">
      <button 
        onClick={onBack}
        className="absolute top-4 left-4 z-10 text-white hover:text-irohGray-200 transition flex items-center gap-2"
      >
        ← Back
      </button>

      <div className="absolute inset-0 bg-black/80">
        <div className="absolute top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2 w-64 h-64 border-2 border-white/50 rounded-lg">
          {/* Transparent scanner window */}
          <div className="absolute inset-2 border border-white/20 rounded-md"></div>
        </div>
      </div>
    </div>
  );
} 