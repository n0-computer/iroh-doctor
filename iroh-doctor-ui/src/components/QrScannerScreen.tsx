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
          const scan = await BarcodeScanner.scan({ windowed: true });
          if (scan.content) {
            const hexString = BigInt(scan.content)
              .toString(16)
              .padStart(64, '0');
            onScan(hexString);
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
    <div className="w-full h-full">
      <button 
        onClick={onBack}
        className="text-irohGray-500 hover:text-irohGray-800"
      >
        ← Back
      </button>
    </div>
  );
} 