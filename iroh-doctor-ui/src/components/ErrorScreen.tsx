import { useLocation, useNavigate } from 'react-router-dom';
import { ScreenWrapper } from './ScreenWrapper';

interface ErrorScreenState {
  message: string;
  details?: string;
}

export function ErrorScreen() {
  const navigate = useNavigate();
  const location = useLocation();
  const { message, details } = (location.state as ErrorScreenState) || {
    message: 'An unknown error occurred'
  };

  return (
    <ScreenWrapper onBack={() => navigate('/')}>
      <div className="flex-col space-y-4">
        <h2 className="text-xl font-semibold text-red-600">Error</h2>
        
        <div className="bg-red-50 border border-red-200 rounded-lg p-4">
          <p className="font-medium text-red-800 mb-2">{message}</p>
          
          {details && (
            <div className="bg-red-100 rounded mt-2 p-3">
              <pre className="whitespace-pre-wrap font-mono text-sm text-red-700 overflow-auto max-h-[200px]">
                {details}
              </pre>
            </div>
          )}
        </div>

        <div className="text-sm text-gray-600 mt-4">
          Try going back and attempting the action again. If the problem persists, 
          please report this issue.
        </div>
      </div>
    </ScreenWrapper>
  );
} 