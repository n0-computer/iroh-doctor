import { useEffect } from 'react';
import * as BarcodeScanner from '@tauri-apps/plugin-barcode-scanner';
import { ScreenWrapper } from './ScreenWrapper';

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
    <ScreenWrapper
      onBack={onBack}
      style={{
        backgroundColor: 'rgba(255, 255, 255, 0.8)',
        clipPath: `polygon(
          0 0, 0 100%, 100% 100%, 100% 0,
          0 0,
          10% 25%, 90% 25%, 90% 75%, 10% 75%,
          10% 25%
        )`
      }}
    />
  );
} 