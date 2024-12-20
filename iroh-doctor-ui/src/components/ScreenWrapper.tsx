import { PropsWithChildren } from 'react';

interface ScreenWrapperProps {
  onBack?: () => void;
  style?: React.CSSProperties;
}

export function ScreenWrapper({ children, onBack, style }: PropsWithChildren<ScreenWrapperProps>) {
  return (
    <div
      className="min-h-screen flex flex-col items-center justify-start p-8 font-space"
      style={style}
    >
      <div className="w-full max-w-md">
        <h1 className="text-[2.5rem] leading-tight font-koulen text-center mb-12 text-irohGray-900">
          iroh doctor
        </h1>

        {onBack && (
          <button
            onClick={onBack}
            className="mb-8 text-irohGray-500 hover:text-irohGray-800 transition flex items-center gap-2"
          >
            ← Back
          </button>
        )}

        {children}

      </div>
    </div>
  );
}
