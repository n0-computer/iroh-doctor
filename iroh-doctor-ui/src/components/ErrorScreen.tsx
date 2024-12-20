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
    message: 'An unknown error occurred',
    details: '<No details available>'
  };

  return (
    <ScreenWrapper onBack={() => navigate('/')}>
      <div className="flex-col space-y-4">
        <div className="bg-red-50 border border-red-200">
          <h2 className="text-lg p-3 font-medium text-red-800 border-b border-red-200">
            Error: {message}
          </h2>
          
          {details && (
            <pre className="p-3 font-mono text-sm text-red-700 bg-red-100 overflow-auto whitespace-nowrap leading-6 h-36">
              {details}
            </pre>
          )}
        </div>

        <p className="text-sm text-gray-600">
          Try going back and attempting the action again. If the problem persists, 
          please report this issue.
        </p>
      </div>
    </ScreenWrapper>
  );
}
