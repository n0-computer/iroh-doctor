import { useState } from 'react'
import { QRCodeSVG } from 'qrcode.react'

interface AcceptingConnScreenProps {
  connectionString: string;
  onBack: () => void;
}

export function AcceptingConnScreen({ connectionString, onBack }: AcceptingConnScreenProps) {
  const [copied, setCopied] = useState(false)
  
  // Construct the full CLI command
  const fullCommand = `iroh-doctor connect ${connectionString}`

  const copyToClipboard = async () => {
    try {
      await navigator.clipboard.writeText(fullCommand)
      setCopied(true)
      setTimeout(() => setCopied(false), 2000)
    } catch (err) {
      console.error('Failed to copy:', err)
    }
  }

  return (
    <div className="w-full">
      <button 
        onClick={onBack}
        className="mb-8 text-irohGray-500 hover:text-irohGray-800 transition flex items-center gap-2"
      >
        ← Back
      </button>

      <div className="space-y-8">
        <div className="space-y-2">
          <label className="text-sm uppercase text-irohGray-500">
            Connect by scanning this QR code
          </label>
          <div className="w-full flex justify-center">
            <div className="bg-white p-3 inline-block">
              <QRCodeSVG 
                value={connectionString}
                size={280}
                level="L"
                includeMargin={false}
              />
            </div>
          </div>
        </div>

        <div className="space-y-2">
          <label className="text-sm uppercase text-irohGray-500">
            Connect via CLI
          </label>
          <div 
            onClick={copyToClipboard}
            className="w-full p-4 bg-irohGray-100 font-spaceMono text-sm cursor-pointer hover:bg-irohGray-200 transition"
          >
            <div className="flex justify-between items-start gap-4">
              <div className="break-all">
                {fullCommand}
              </div>
              <div className="text-xs uppercase text-irohGray-500 whitespace-nowrap">
                {copied ? 'Copied!' : 'Click to copy'}
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  )
} 