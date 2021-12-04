pub mod sites {
    pub mod v1beta1 {
        tonic::include_proto!("fairing.sites.v1beta1");
    }
}

pub mod sources {
    pub mod v1beta1 {
        tonic::include_proto!("fairing.sources.v1beta1");
    }
}

pub mod teams {
    pub mod v1beta1 {
        tonic::include_proto!("fairing.teams.v1beta1");
    }
}

pub mod users {
    pub mod v1beta1 {
        tonic::include_proto!("fairing.users.v1beta1");
    }
}
