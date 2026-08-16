    use super::{ConnectorError, prepare_error_is_already_terminal};

    #[test]
    fn already_terminal_prepare_is_recognized() {
        let error = ConnectorError::new(
            "already_terminal: attempt journal already crossed dispatch or is terminal",
        );
        assert!(prepare_error_is_already_terminal(&error));
        assert!(!prepare_error_is_already_terminal(&ConnectorError::new(
            "aggregate dispatch deadline or cancellation"
        )));
    }

